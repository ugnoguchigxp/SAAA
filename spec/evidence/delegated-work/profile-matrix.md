# Delegated profile × operation matrix (DWR-08)

Recorded 2026-09-21 against pinned Pi 0.86.1 and the existing Codex SDK adapter.

| Profile | read | test_run | write | network | notes |
| --- | --- | --- | --- | --- | --- |
| `delegated-codex-sdk-macos-v1` | supported (workspace README) | unsupported as a distinct tool set | rejected by SDK read-only sandbox | rejected | Built-in shell cannot be scoped per operation; prompt self-restraint is not counted. Strict test recipes go through host `recipe_runner`. |
| `delegated-read-test-macos-v1` | broad `file-read*` | process allowed | temp write only | denied | Not advertised as a narrow read scope. |
| Host `delegated_profile` adapter | workspace canonical read/list/search | n/a | rejected | rejected | Symlink escape and `.saaa/delegated-sdk-state` are denied at the host. |

SDK cannot force a per-tool allowlist that splits read from test without a new Coding Agent. Strict delegated test execution uses registered recipes (DWR-10).
