//! Per-connection request handling and command dispatch.

use std::io::BufReader;

use interprocess::local_socket::prelude::*;

use locus_core::context::ContextBriefOptions;
use locus_core::ipc::protocol::{
    command, error_code, ConflictsRequest, ConflictsResponse, ContextRequest, ContextResponse,
    ForgetRequest, ForgetResponse, PingResponse, ReindexResponse, RememberRequest,
    RememberResponse, Request, Response, SearchRequest, SearchResponse, MAX_MESSAGE_BYTES,
    PROTOCOL_VERSION,
};
use locus_core::ipc::wire::{read_message, write_message, ReadOutcome};
use locus_core::memory::{MemoryType, NewMemory};
use locus_core::search::Query;
use locus_core::Error;

use crate::server::Shared;
use crate::writer::{WriterOk, WriterOp};

/// Close a connection after this many malformed messages in a row.
const MAX_MALFORMED: u32 = 3;

/// Reads and answers requests on a single connection until it closes.
pub fn handle_connection(shared: &Shared, stream: LocalSocketStream) {
    let mut reader = BufReader::new(&stream);
    let mut malformed: u32 = 0;

    loop {
        match read_message(&mut reader, MAX_MESSAGE_BYTES) {
            Ok(ReadOutcome::Message(bytes)) => {
                let response = dispatch(shared, &bytes, &mut malformed);
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

fn dispatch(shared: &Shared, bytes: &[u8], malformed: &mut u32) -> Response {
    let request: Request = match serde_json::from_slice(bytes) {
        Ok(request) => request,
        Err(_) => {
            *malformed += 1;
            return Response::error("", error_code::MALFORMED_JSON, "request was not valid JSON");
        }
    };
    *malformed = 0;

    if request.v != PROTOCOL_VERSION {
        return Response::error(
            request.id,
            error_code::UNSUPPORTED_VERSION,
            format!("unsupported protocol version: {}", request.v),
        );
    }

    shared.begin_request();
    let response = route(shared, &request);
    shared.end_request();
    response
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

    match shared.store().search(query) {
        Ok(hits) => ok(
            request,
            &SearchResponse {
                count: hits.len(),
                hits,
            },
        ),
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

    match shared.store().context_brief(query, options) {
        Ok(brief) => ok(request, &ContextResponse { brief }),
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

    match shared.writer().submit(WriterOp::Remember(new_memory)) {
        Ok(WriterOk::Remembered(id)) => ok(request, &RememberResponse { id }),
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
