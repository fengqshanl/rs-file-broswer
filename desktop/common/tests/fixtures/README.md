# Test fixtures

`test_host_key` is a throwaway Ed25519 SSH host key generated for the
in-process test server used by `tests/ssh_e2e.rs` and `tests/ssh_companion.rs`.
It protects nothing, is committed on purpose so the tests need no `ssh-keygen`,
and is never used by the shipped application.
