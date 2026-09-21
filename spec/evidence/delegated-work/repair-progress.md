# Delegated work repair progress

HEAD at start: `f4ad46ddc36042fc419b175186ca7f5572741c24` (2026-09-21). Schema version 31. rustc 1.92.0, Xcode 26.3, Node 24.11.1, bun 1.3.14.

A01–A12: code+automated tests. Live GUI/model/sleep remain external-wait (A11 UI path is host+panel tests only).

| Card | Status | Evidence |
| --- | --- | --- |
| DWR-00 | complete | this file + `repair-results.md` |
| DWR-01 | complete | `dw_r01_*` |
| DWR-02a | complete | `dw_r02_contract_*` / `dw_r02_plan_*` |
| DWR-02b | complete | schema 31 + `dw_r02_migration_*` |
| DWR-03 | complete | `dw_r03_*` |
| DWR-04 | complete | `dw_r04_*` |
| DWR-05 | complete | `dw_r05_*` |
| DWR-06 | complete | `commit_delegated_job` + `dw_r06_*` |
| DWR-07 | complete | queue/park + `dw_r07_*` |
| DWR-08 | complete | `profile-matrix.md` |
| DWR-09 | complete | `dw_r09_*` |
| DWR-10 | complete | `dw_r10_*` |
| DWR-11 | complete | budget remaining + `dw_r11_*` |
| DWR-12 | complete | `dw_r12_*` |
| DWR-13 | complete | plans + `dw_r13_*` |
| DWR-14a | complete | dispatch gates |
| DWR-14b | complete | revoke-before-dispatch |
| DWR-14c | complete | `dw_r14_forget_*` |
| DWR-15 | complete | `dw_r15_*` |
| DWR-16 | complete | pump drain + schedule select |
| DWR-17 | complete | wake bind on start_loop |
| DWR-18 | complete | `dw_r18_*` |
| DWR-19a | complete | `dw_r19_*` |
| DWR-19b | complete | confirmation/recipe UI + panel test |
| DWR-20 | complete | `delegated-report-committed` + chat test |
| DWR-21 | complete | corpus 40 cases + `dw_r21_*` scripted lane |
| DWR-22 | complete | `dw_r22_*` (fixture pump; live Pi remaining in 24) |
| DWR-23 | external-wait | WDIO plugin not in production Cargo features; Accessibility not granted |
| DWR-24 | external-wait | authenticated model, GUI session, sleep/wake not demonstrated |
| DWR-25 | complete (fixture) | `dw_r25_*` n=32 driver/drain p95 ≤ 2s; live app not measured |
| DWR-26 | complete with holds | steward size baselines registered; other-work size/check:local still fail separately |
