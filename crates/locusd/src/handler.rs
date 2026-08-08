//! Per-connection request handling and command dispatch.

use std::io::BufReader;
use std::sync::mpsc::RecvTimeoutError;
use std::time::Duration;

use interprocess::local_socket::prelude::*;

use locus_core::capture::CaptureTrigger;
use locus_core::context::ContextBriefOptions;
use locus_core::ipc::protocol::{
    command, error_code, CaptureRequest, CaptureResponse, ConflictsRequest, ConflictsResponse,
    ContextRequest, ContextResponse, EventsRequest, EventsResponse, ForgetRequest, ForgetResponse,
    PingResponse, ReindexResponse, RememberRequest, RememberResponse, Request, Response,
    SearchRequest, SearchResponse, Warning, MAX_MESSAGE_BYTES, PROTOCOL_VERSION,
};
use locus_core::ipc::wire::{read_message, write_message, ReadOutcome};
use locus_core::memory::{MemoryType, NewMemory};
use locus_core::search::Query;
use locus_core::Error;

use crate::server::Shared;
use crate::writer::{memory_event, WriterOk, WriterOp};

/// Close a connection after this many malformed messages in a row.
const MAX_MALFORMED: u32 = 3;

/// How often the live event stream wakes to check for daemon shutdown and how
/// long a single event write may block before the peer is considered hung.
const EVENT_POLL_INTERVAL: Duration = Duration::from_millis(250);
const EVENT_SEND_TIMEOUT: Duration = Duration::from_secs(2);

/// Outcome of dispatching one request.
enum Dispatch {
    /// A normal request/response exchange to write to the connection.
    Response(Response),
    /// The `events` request: write the ack, then stream live events until the
    /// connection closes or the daemon shuts down.
    Events(Response),
}

/// Reads and answers requests on a single connection until it closes.
pub fn handle_connection(shared: &Shared, stream: LocalSocketStream) {
    let mut reader = BufReader::new(&stream);
    let mut malformed: u32 = 0;

    loop {
        match read_message(&mut reader, MAX_MESSAGE_BYTES) {
            Ok(ReadOutcome::Message(bytes)) => match dispatch(shared, &bytes, &mut malformed) {
                Dispatch::Response(response) => {
                    if !write_response(&stream, &response) {
                        return;
                    }
                    if malformed >= MAX_MALFORMED {
                        shared
                            .log()
                            .warn("closing connection after repeated malformed input");
                        return;
                    }
                    if shared.is_shutdown() {
                        return;
                    }
                }
                Dispatch::Events(ack) => {
                    if write_response(&stream, &ack) {
                        handle_events(shared, &stream);
                    }
                    return;
                }
            },
            Ok(ReadOutcome::TooLarge) => {
                let response = Response::error(
                    "",
                    error_code::MESSAGE_TOO_LARGE,
                    "message exceeds the maximum allowed size",
                );
                let _ = write_response(&stream, &response);
                return;
            }
            Ok(ReadOutcome::Eof) => return,
            Err(_) => return,
        }
    }
}

fn write_response(stream: &LocalSocketStream, response: &Response) -> bool {
    let bytes = match serde_json::to_vec(response) {
        Ok(bytes) => bytes,
        Err(_) => return false,
    };
    let mut writer = stream;
    write_message(&mut writer, &bytes).is_ok()
}

fn write_event(stream: &LocalSocketStream, event: &locus_core::ipc::protocol::MemoryEvent) -> bool {
    let bytes = match serde_json::to_vec(event) {
        Ok(bytes) => bytes,
        Err(_) => return false,
    };
    let mut writer = stream;
    write_message(&mut writer, &bytes).is_ok()
}

/// Streams live memory events to a subscriber until the connection breaks or
/// the daemon shuts down. A hung peer (one that stops reading) fails its write
/// after [`EVENT_WRITE_TIMEOUT`] and is unsubscribed.
fn handle_events(shared: &Shared, stream: &LocalSocketStream) {
    let _ = stream.set_send_timeout(Some(EVENT_SEND_TIMEOUT));
    shared.begin_request();
    let (subscriber_id, rx) = shared.events().subscribe();
    loop {
        match rx.recv_timeout(EVENT_POLL_INTERVAL) {
            Ok(event) => {
                if !write_event(stream, &event) {
                    break;
                }
            }
            Err(RecvTimeoutError::Timeout) => {
                if shared.is_shutdown() {
                    break;
                }
            }
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
    shared.events().unsubscribe(subscriber_id);
    shared.end_request();
}

/// Records access and publishes live events for memories that were surfaced.
///
/// Non-blocking on both paths: the writer op is fire-and-forget (never waited
/// on) and [`EventBus::publish`] never blocks, so a slow live viewer cannot
/// stall the request.
fn surface(
    shared: &Shared,
    kind: locus_core::ipc::protocol::MemoryEventKind,
    memories: &[locus_core::memory::Memory],
) {
    if memories.is_empty() {
        return;
    }
    let ids = memories
        .iter()
        .map(|memory| memory.id.clone())
        .collect::<Vec<_>>();
    for memory in memories {
        shared.events().publish(&memory_event(kind, memory, 1));
    }
    shared.writer().submit_async(WriterOp::RecordAccess(ids));
}

fn dispatch(shared: &Shared, bytes: &[u8], malformed: &mut u32) -> Dispatch {
    let request: Request = match serde_json::from_slice(bytes) {
        Ok(request) => request,
        Err(_) => {
            *malformed += 1;
            return Dispatch::Response(Response::error(
                "",
                error_code::MALFORMED_JSON,
                "request was not valid JSON",
            ));
        }
    };
    *malformed = 0;

    if request.v != PROTOCOL_VERSION {
        return Dispatch::Response(Response::error(
            request.id,
            error_code::UNSUPPORTED_VERSION,
            format!("unsupported protocol version: {}", request.v),
        ));
    }

    if request.cmd == command::EVENTS {
        if let Err(response) = parse_payload::<EventsRequest>(&request) {
            return Dispatch::Response(*response);
        }
        let ack = ok(&request, &EventsResponse { subscribed: true });
        return Dispatch::Events(ack);
    }

    shared.begin_request();
    let response = route(shared, &request);
    shared.end_request();
    Dispatch::Response(response)
}

fn route(shared: &Shared, request: &Request) -> Response {
    match request.cmd.as_str() {
        command::PING => ok(
            request,
            &PingResponse {
                version: locus_core::VERSION.to_string(),
                protocol: PROTOCOL_VERSION,
            },
        ),
        command::STATUS => ok(request, &shared.status_snapshot()),
        command::SEARCH => handle_search(shared, request),
        command::CONTEXT => handle_context(shared, request),
        command::REMEMBER => handle_remember(shared, request),
        command::FORGET => handle_forget(shared, request),
        command::REINDEX => handle_reindex(shared, request),
        command::CONFLICTS => handle_conflicts(shared, request),
        command::CAPTURE => handle_capture(shared, request),
        command::STOP => {
            shared.log().info("stop requested by client");
            shared.request_shutdown();
            ok(request, &serde_json::json!({ "stopping": true }))
        }
        other => Response::error(
            request.id.clone(),
            error_code::UNKNOWN_COMMAND,
            format!("unknown command: {other}"),
        ),
    }
}

fn handle_search(shared: &Shared, request: &Request) -> Response {
    let payload: SearchRequest = match parse_payload(request) {
        Ok(payload) => payload,
        Err(response) => return *response,
    };

    let memory_type = match parse_type_filter(request, payload.memory_type.as_deref()) {
        Ok(memory_type) => memory_type,
        Err(response) => return *response,
    };

    let query = Query {
        text: payload.text,
        namespace: payload.namespace,
        memory_type,
        limit: payload.limit,
    };

    match shared.store().retrieve(query) {
        Ok(outcome) => {
            surface(
                shared,
                locus_core::ipc::protocol::MemoryEventKind::Searched,
                &outcome.memories,
            );
            ok(
                request,
                &SearchResponse {
                    count: outcome.hits.len(),
                    hits: outcome.hits,
                },
            )
        }
        Err(err) => error_response(shared, request, err),
    }
}

fn handle_context(shared: &Shared, request: &Request) -> Response {
    let payload: ContextRequest = match parse_payload(request) {
        Ok(payload) => payload,
        Err(response) => return *response,
    };

    let memory_type = match parse_type_filter(request, payload.memory_type.as_deref()) {
        Ok(memory_type) => memory_type,
        Err(response) => return *response,
    };

    let query = Query {
        text: payload.text,
        namespace: payload.namespace,
        memory_type,
        limit: payload.limit,
    };

    let options = match payload.token_budget {
        Some(token_budget) => ContextBriefOptions { token_budget },
        None => ContextBriefOptions::default(),
    };

    match shared.store().context_brief_with_memories(query, options) {
        Ok((brief, memories)) => {
            surface(
                shared,
                locus_core::ipc::protocol::MemoryEventKind::Used,
                &memories,
            );
            ok(request, &ContextResponse { brief })
        }
        Err(err) => error_response(shared, request, err),
    }
}

fn handle_remember(shared: &Shared, request: &Request) -> Response {
    let payload: RememberRequest = match parse_payload(request) {
        Ok(payload) => payload,
        Err(response) => return *response,
    };

    let memory_type = match MemoryType::parse(&payload.memory_type) {
        Ok(memory_type) => memory_type,
        Err(err) => return error_response(shared, request, err),
    };

    let new_memory = NewMemory {
        namespace: payload.namespace,
        memory_type,
        title: payload.title,
        content: payload.content,
        entities: payload.entities,
        importance: payload.importance,
        source: payload.source,
    };

    match shared
        .writer()
        .submit(WriterOp::Remember(new_memory, payload.allow_secret))
    {
        Ok(WriterOk::Remembered(id, warnings)) => {
            ok_with_warnings(request, &RememberResponse { id }, warnings)
        }
        Ok(_) => internal(shared, request, "unexpected writer result"),
        Err(err) => error_response(shared, request, err),
    }
}

fn handle_forget(shared: &Shared, request: &Request) -> Response {
    let payload: ForgetRequest = match parse_payload(request) {
        Ok(payload) => payload,
        Err(response) => return *response,
    };

    let id = payload.id.clone();
    match shared.writer().submit(WriterOp::Forget(payload.id)) {
        Ok(WriterOk::Forgotten) => ok(request, &ForgetResponse { id }),
        Ok(_) => internal(shared, request, "unexpected writer result"),
        Err(err) => error_response(shared, request, err),
    }
}

fn handle_reindex(shared: &Shared, request: &Request) -> Response {
    match shared.writer().submit(WriterOp::Reindex) {
        Ok(WriterOk::Reindexed(reindexed)) => ok(request, &ReindexResponse { reindexed }),
        Ok(_) => internal(shared, request, "unexpected writer result"),
        Err(err) => error_response(shared, request, err),
    }
}

fn handle_capture(shared: &Shared, request: &Request) -> Response {
    let payload: CaptureRequest = match parse_payload(request) {
        Ok(payload) => payload,
        Err(response) => return *response,
    };

    let trigger = CaptureTrigger {
        namespace: payload.namespace,
        text: payload.text,
    };

    match shared.writer().submit(WriterOp::Capture(trigger)) {
        Ok(WriterOk::Captured(outcome)) => ok(
            request,
            &CaptureResponse {
                written: outcome.written,
                skipped_tasks: outcome.skipped_tasks,
                skipped_duplicates: outcome.skipped_duplicates,
            },
        ),
        Ok(_) => internal(shared, request, "unexpected writer result"),
        Err(err) => error_response(shared, request, err),
    }
}

fn handle_conflicts(shared: &Shared, request: &Request) -> Response {
    let payload: ConflictsRequest = match parse_payload(request) {
        Ok(payload) => payload,
        Err(response) => return *response,
    };

    match shared.store().list_conflicts(payload.namespace) {
        Ok(conflicts) => ok(
            request,
            &ConflictsResponse {
                count: conflicts.len(),
                conflicts,
            },
        ),
        Err(err) => error_response(shared, request, err),
    }
}
fn parse_payload<T>(request: &Request) -> std::result::Result<T, Box<Response>>
where
    T: serde::de::DeserializeOwned,
{
    serde_json::from_value(request.payload.clone()).map_err(|err| {
        Box::new(Response::error(
            request.id.clone(),
            error_code::INVALID_INPUT,
            format!("invalid payload: {err}"),
        ))
    })
}

fn parse_type_filter(
    request: &Request,
    value: Option<&str>,
) -> std::result::Result<Option<MemoryType>, Box<Response>> {
    match value {
        None => Ok(None),
        Some(raw) => MemoryType::parse(raw).map(Some).map_err(|err| {
            Box::new(Response::error(
                request.id.clone(),
                error_code::INVALID_INPUT,
                err.to_string(),
            ))
        }),
    }
}

fn ok<T: serde::Serialize>(request: &Request, payload: &T) -> Response {
    match serde_json::to_value(payload) {
        Ok(value) => Response::ok(request.id.clone(), value, Vec::new()),
        Err(_) => Response::error(
            request.id.clone(),
            error_code::INTERNAL,
            "failed to encode response",
        ),
    }
}

fn ok_with_warnings<T: serde::Serialize>(
    request: &Request,
    payload: &T,
    warnings: Vec<Warning>,
) -> Response {
    match serde_json::to_value(payload) {
        Ok(value) => Response::ok(request.id.clone(), value, warnings),
        Err(_) => Response::error(
            request.id.clone(),
            error_code::INTERNAL,
            "failed to encode response",
        ),
    }
}

fn error_response(shared: &Shared, request: &Request, err: Error) -> Response {
    match err {
        Error::InvalidInput(message) => {
            Response::error(request.id.clone(), error_code::INVALID_INPUT, message)
        }
        Error::NotFound(message) => {
            Response::error(request.id.clone(), error_code::NOT_FOUND, message)
        }
        other => internal(shared, request, &other.to_string()),
    }
}

fn internal(shared: &Shared, request: &Request, detail: &str) -> Response {
    // Log the raw detail but never leak it over the wire.
    shared.log().error(&format!("internal error: {detail}"));
    shared.set_last_error(detail.to_string());
    Response::error(request.id.clone(), error_code::INTERNAL, "internal error")
}
