# Vendored secret-detection rules

This directory contains static, vendored secret-detection rules embedded into
`locus-core` at build time (see `crates/locus-core/src/security.rs`). Rules are
loaded once into a process-wide compiled set; nothing is fetched at runtime.

## Source and license

`gitleaks-subset.toml` is a **curated subset** of the default gitleaks
configuration, copied character-for-character from:

- Source: <https://github.com/gitleaks/gitleaks>
- File: `config/gitleaks.toml`
- License: **MIT** (see `gitleaks-MIT-LICENSE.txt` below)

The subset includes the AWS, GitHub, private-key, GCP, Slack, Stripe, npm,
SendGrid, Twilio, JWT, PyPI, OpenAI, and Anthropic rules used by U-011. It
deliberately omits entropy-gated generic rules (e.g. `generic-api-key`) whose
large allowlists/stopword tables are tuned for whole-repo scanning and would
produce false positives on short memory text.

### Faithful deviations

Two regexes are normalized from the upstream text (semantically equivalent,
same charset as Go's `\w`, i.e. `[0-9A-Za-z_]`), because Rust's `regex` crate
treats `\w` as Unicode-aware, producing an automaton that exceeds the compiled
size limit:

- `pypi-upload-token`: `[\w-]{50,1000}` → `[0-9A-Za-z_-]{50,1000}`
- `github-fine-grained-pat`: `\w{82}` → `[0-9A-Za-z_]{82}`

### Curated addition (not from gitleaks)

One rule is not part of gitleaks:

- `password-in-url` — flags credentials embedded in a URL
  (`scheme://user:password@host`). Modeled on the `URL_CREDENTIALS` regex from
  the detect-secrets project (<https://github.com/Yelp/detect-secrets>,
  Apache-2.0). Added because gitleaks ships no dedicated password-in-URL rule
  and U-011 requires the pattern to be flagged.

## Scope note

Locus does not ship the full gitleaks rule set. The full set targets whole
repository scanning; Locus scans short memory title/content fields where
entropy-based generic rules would over-flag benign text. If the rule set grows,
keep it in this directory and update this README.
