# Adaptive improvement evaluation protocol

An artifact starts as `candidate` and may be activated only after an external evaluator has moved
it to `eligible`. The evaluator must use grouped, time-separated data and preserve the same
candidates, budget, verifier, and hard constraints as rules. It must record the preselected main
metric and a paired bootstrap confidence interval before calling activation.

The runtime recomputes the candidate fingerprint at dispatch. A mismatch, invalid artifact, or
missing active activation falls back to rules. Explicit scope-bound corrections take precedence
and do not wait for evaluation.
