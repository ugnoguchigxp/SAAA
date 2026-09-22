# Artifact WebView performance results — 2026-09-22

Same build mode was not used for a before/after desktop memory capture in this session. Numbers below are contract limits and unit-test observations.

## Limits

- Active child WebView: 1
- Artifact tabs: 8
- HTML payload: 1 MiB
- Preview token: one per prepare, TTL 120s
- Geometry updates: coalesced to animation frame
- Create timeout: 5s

## Observations

- Lifecycle unit test: switching artifacts closes the previous mock WebView before creating the next.
- Token ledger: filling 8 tokens then advancing the clock past TTL returns the count to 0.
- Orphan recovery: failed UI cleanup is idempotent; expired tokens are dropped on access.

## Not measured here

Main window RSS before/after 100 live tab switches, cold WebView start, maximize/minimize. Those remain WVP-12 / WVP-14 items.
