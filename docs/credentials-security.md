# Credential security

System declarations contain credential names such as `login-owner` and `login-root`,
never plaintext passwords or password hashes. Credentials are persistent machine
state, independent of the selected release.

## Storage and authentication

- Protected credential files are stored under `/var/lib/looom/credentials`.
  Private directories are root-owned with `0700`; files use `0600`.
- Release account templates contain locked password fields. At boot, the Rust
  credential service generates runtime `shadow` and `gshadow` under `/run` with `0600`.
- PAM authenticates against these runtime files. There is no custom looom login protocol.
- New passwords use the system libxcrypt implementation of yescrypt with `j9T`
  parameters. Bounded legacy SHA-512-crypt hashes can be imported.
- File access checks owner, type, permissions, trusted parent directories,
  symlinks, and access/default ACLs using checked descriptors.
- Password updates and runtime generation share a lock. Persistent writes use
  synchronization, atomic replacement, and recovery metadata. Password buffers
  are zeroized and credential-process core dumps are disabled.

A changed password remains current after switching or rolling back system releases.

## Change a password

Use `looom setup` as your regular user, or run:

```sh
sudo looom password <user-name>
```

The command asks for the new password twice with hidden input. For controlled
automation, `--stdin` reads it from standard input; keep the source private and
never put a password in command arguments, YAML, shell history, or public logs.
`looom-password` is a compatibility entry point to the same Rust manager.

Direct changes through `passwd`, `usermod`, desktop password dialogs, or self-service
PAM password changes are not supported. Full password aging and persistent account
locking policies are also outside the current implementation.

## SSH keys

SSH server host keys identify the installed computer and persist in
`/var/lib/looom/ssh`. They are created even when no user public key was supplied.
They do not grant login access. A supplied public key is installed into the managed
accounts' `authorized_keys`; private client keys stay on the client.
Installed read-only releases disable password-based SSH login.

## Backups and limits

Root can read or replace password hashes. Offline access to an unencrypted disk
can also expose them; filesystem permissions do not provide disk encryption.
Hashes cannot be decrypted, but leaked hashes can be attacked by password guessing.

Recovery roots, previous recovery generations, installer workspaces, State backups,
SSH private host keys, and NetworkManager connection files can contain secrets.
Protect them separately and do not include them in source archives or public evidence.
Package reproducibility does not require publishing credentials.

The implementation has VM tests for permissions, ACLs, symlinks, concurrency,
interrupted writes, and persistence. These are functional checks, not an independent
cryptographic or PAM security audit. See [release validation](runs/2026-10-12-release-0.2.0/README.md).
