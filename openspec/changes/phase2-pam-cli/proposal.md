# Proposal: phase2-pam-cli

## Change name
`phase2-pam-cli`

## Status
`proposed`

## What

Complete dax-auth so a real user can enroll their face and authenticate via PAM — end to end.

Phase 1 delivered a compilable skeleton with ML infrastructure, encrypted store, the IPC wire protocol, and a working ONNX inference pipeline. Phase 2 closes the remaining gaps:

1. **Threshold bug fix** (`session.rs:115`) — hardcoded `0.65` must come from config
2. **Model distribution** — download script + README for the three ONNX files
3. **CLI: enroll** — the one command that makes the system usable (capture → embed → store)
4. **CLI: list / remove / clear / test / status** — full management UX
5. **PAM module: `authenticate_inner()`** — the actual OS integration point
6. **Umeyama face alignment** — replace bbox-crop with proper 5-point similarity transform for ArcFace accuracy

## Why this order

```
bug-fix → models → enroll → pam
```

- You cannot enroll without models → models must come first
- You cannot test PAM without an enrolled face → enroll must come before PAM
- The threshold bug must be fixed before PAM is implemented to avoid hardcoded security policy
- Umeyama alignment improves embedding quality for ALL subsequent operations, so it belongs early in Phase 2

## Success criteria

A real user on a fresh Linux install can:

1. Run `scripts/download_models.sh` and get all three models, SHA-256 verified
2. Run `sudo dax-auth enroll` and have their face stored encrypted on disk
3. Invoke `sudo <anything>` and have PAM authenticate via face (not password)
4. Run `dax-auth list` to see enrolled faces with dates
5. Run `dax-auth remove 0` to remove a specific face
6. Run `dax-auth test` to get a full diagnostic report (camera, detection, liveness, score)
7. Run `dax-auth status` to confirm the daemon is running

No hardcoded thresholds anywhere. Security mode comes from `/etc/dax-auth/config.toml`.

## Out of scope for Phase 2

- GUI enrollment wizard
- Windows hello IR camera depth liveness (deferred — hardware not available)
- VitisAI NPU execution provider (deferred — ort rc.12 bug)
- Multi-user concurrent authentication (daemon serializes via Mutex — acceptable for Phase 2)
- Packaging (`.deb`, `.rpm`) — Phase 3
