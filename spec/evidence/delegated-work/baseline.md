# Delegated work baseline

## Observed starting point

- Steward work was tied to two exact Japanese trigger strings and was only
  inspected from a subsequent conversation turn.
- `steward_one_active_goal` limited each conversation to one active goal.
- Coding already persisted job lifecycle events, but steward had no durable
  cursor over them; a completed job alone did not wake report handling.
- Coding has one active-run constraint.  The delegated-work implementation
  preserves that execution limit while allowing multiple queued goals.

## Boundaries retained

- Delegated work only grants `read` and registered `test_run` operations.
- No delegated path is allowed to grant write, arbitrary shell, or network
  access through text interpretation.
- Existing Coding job/recovery state remains the execution authority.
