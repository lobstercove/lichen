# Checkpoint catalog admission latency

Candidate based on signed v0.5.284, commit
`7a74b124b30a5bcdca4dcbf1603195b6cab678b6`. Not a deployed release.

The September 7 live cadence observation failed after Singapore spent 3,080 ms
in checkpoint admission at block 12,618,000. The other three validators timed
out proposing height 12,618,001; the network recovered at round one. The retained
cadence failure is `ed75ce4c185ed1b76a8d2811779b2f28df236aa9f10d034b75b568c270dd5358`;
the Singapore diagnostic is
`e2634e490a839a8be698d0a54897ff768818ece5b29fe647d15eebf658910189`.

`maybe_create_checkpoint` resolved the authenticated profile at each legacy
1,000-slot boundary before testing the profile's 10,000-slot cadence. Resolving
the handoff repeatedly validated the same catalog through nested coverage and
root calls. The installed catalog has 417 segments and 44,526,657 encoded bytes.

The candidate checks the cadence before resolving the profile. The original
cheap legacy boundary check still precedes the reader lookup. Legacy and
preactivation checkpoints retain their existing cadence. Archive readers own a
catalog validated by `ArchiveV2Catalog::load`, with no mutable accessor; their
handoff method reuses that validation while checking coverage and computing the
same domain-separated prefix root. Public mutable-catalog entry points still
perform full validation. The state handoff retains one reader reference for the
whole calculation, preserving fresh-sync admission and unpublished-tail bounds.
No catalog format, hash algorithm, history-retention limit, disk reserve or
consensus transition is changed.

Regression coverage includes invalid coverage, empty/genesis handoffs, corrupt
catalog roots, file replacement after reader admission, corrupt replacement on
reopen, pre-admission rejection, unpublished-tail limits, append-stable roots,
and scheduling across two full checkpoint intervals. An explicitly enabled
production-fixture test compares the checked catalog path and admitted reader
at every segment boundary, declared-gap boundary, genesis and an uncovered end.
It reports elapsed costs without imposing a machine-dependent timing assertion.

Final focused core tests: four passed, including the production fixture.
All 422 boundary results matched for the 417-segment catalog. On the local Mac,
the checked catalog path took 78,763,374 microseconds in total; the admitted
reader took 18,623 microseconds. These are local aggregate measurements, not
live validator latency acceptance. All 64 validator checkpoint tests passed.
The broader Archive suite passed 75 tests; its one ignored production-fixture
test is the explicitly enabled, separately passed test above.
The fixture must match SHA-256
`9568fd7a926f61eb05361722293e35718e9e00621be1960094f5610922b5d255`
for the September 7 417-segment evidence. Run from the candidate checkout:

```sh
cargo fmt --all -- --check
cargo test --release -p lobstercove-lichen-core --lib --all-features --locked checkpoint_handoff
# Set LICHEN_TEST_HANDOFF_CATALOG to the verified retained fixture path first.
cargo test --release -p lobstercove-lichen-core --lib --all-features --locked checkpoint_handoff -- --include-ignored --nocapture
cargo test --release -p lichen-validator --all-features --locked checkpoint_
```

Full workspace, security, contract, frontend and four-validator release gates
remain mandatory before a signed replacement. A fresh live cadence observation
must retain the previous failed interval as incident evidence. This correction
does not establish full logical-history parity, physical retirement, checkpoint
capacity on every host, or durable source credential renewal.
