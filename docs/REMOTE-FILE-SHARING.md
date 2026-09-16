# Remote file sharing over the tailnet

How a human reaches a remote machine's files from a Mac Finder — SMB gated to
the tailnet, staged by an agent, installed by the owner with one sudo command.

This is a human surface, not a control plane. Agent-to-agent work goes through
the Control Service (`CROSS-CELL-CONNECTIONS.md`); admin access goes through
SSH, which remains the bootstrap transport and never becomes an API
(`CONNECTIVITY-FABRIC.md`). This runbook exists because neither of those gives
a person a Finder window, and the first time it was needed it cost a full
session of dead ends. Everything below was proven live: Mac (macOS 26) ⇄
omarchy machine `frank` (`100.92.62.101`, tailnet name `frank.tail7e55a2.ts.net`),
2026-09-16.

## Why SMB, not Finder's SFTP

Finder's `sftp://` client is unreliable on current macOS. The proven failure:
terminal `ssh` and terminal `sftp` to the same host both connected cleanly with
the same credentials, while Finder's ⌘K `sftp://` attempt failed with a
misleading file-error dialog against a healthy OpenSSH 10.5 server. Do not
diagnose the server when only Finder fails; diagnose Finder's client by
running `sftp <user>@<host>` in a terminal first, then move to SMB, which is
the file-sharing path Apple actually maintains.

## Design law

- **Gate access at the protocol layer, not the interface layer.**
  `bind interfaces only = yes` with a Tailscale address silently fails: a /32
  point-to-point peer address does not match Samba's interface matcher, and the
  daemon falls back to binding loopback only — with no error at start-up. The
  working pattern is to listen on all interfaces and refuse every peer outside
  loopback and the tailnet ranges (`hosts allow`).
- **The LISTEN line is ground truth.** After any Samba change, `ss -tln` on the
  remote must show `0.0.0.0:445` (or `*:445`). A line bound to `127.0.0.1:445`
  means no remote machine can connect, whatever the service status says.
- **SMB credentials are a separate store.** `smbpasswd -a <user>` registers the
  user in Samba's own password database; the login password is irrelevant to
  it, and an account that only ever authenticated by SSH key may have no usable
  password at all until one is set.
- **The staged one-shot pattern is how root gets involved exactly once.**
  An agent holding a key-authorised SSH alias (no interactive password, usually
  no passwordless sudo) stages the config and an installer script into the
  remote home directory; the owner runs one `ssh -t <alias> 'sudo sh …'`
  command and types their own password. Neither the sudo password nor the
  chosen SMB password ever travels through the agent.

## The recipe

Automated staging:

```bash
scripts/stage-remote-file-share.sh <ssh-alias> [smb-user]
```

The script checks reachability and that `samba`/`smbclient` are installed,
stages `~/smb.conf.staged` and `~/enable-file-share.sh` into the remote home,
and prints the exact sudo command for the owner. It needs a working SSH key
alias (see `Host oi-omarchy` in the Mac's `~/.ssh/config` for the pattern:
dedicated key, `IdentitiesOnly`, `StrictHostKeyChecking yes`) and needs no
root where it runs.

For in-process use, `crates/workcell-fileshare` stages the same material under
the `ServiceProvider` port (`provider:fileshare-smb`): the share blueprint is
provider-native, the request stays a semantic connection requirement, and
`observe` reports what is actually on disk — staging is not installation there
either.

Manual equivalent — the config that is staged:

```ini
[global]
   server role = standalone server
   server min protocol = SMB2
   map to guest = never

   # Listen everywhere; only loopback and the tailnet may talk.
   # 100.64.0.0/10 = Tailscale IPv4, fd7a:115c:a1e0::/48 = Tailscale IPv6.
   hosts allow = 127.0.0.0/8 100.64.0.0/10 fd7a:115c:a1e0::/48
   hosts deny = ALL

   log file = /var/log/samba/%m.log

[homes]
   comment = Home on %h
   browseable = no
   valid users = %S
   writable = yes
```

On Arch, Samba ships no default `smb.conf` and `smb.service` refuses to start
without one — an empty `/etc/samba/` is the usual "samba didn't work" cause.
The installer installs the config, runs `smbpasswd -a <user>` (owner chooses
the password interactively, typed twice), then
`systemctl enable --now smb.service`.

Then from the Mac: Finder ⌘K → `smb://<host>` (the tailnet name or 100.x
address), authenticate as the SMB user with the smbpasswd password. The home
directory mounts as a volume.

## Diagnostic ladder

Run in this order; each rung names the failure it catches.

```text
tailscale status                     path down, or peer offline — nothing else matters yet
nc -z -G 3 <peer-ip> 22              no SSH at all: no key route to stage anything
ssh -o BatchMode=yes <alias> true    key not authorised (denied here ≠ server broken)
ssh -o PreferredAuthentications=none <user>@<host>
                                     server names the auth methods it accepts;
                                     "publickey,password" means password logins are on
nc -z -G 3 <peer-ip> 445             SMB not reachable: service down, or bound loopback-only
ssh <alias> ss -tln | grep 445       the LISTEN line — see design law above
sftp <user>@<host>                   terminal-side SFTP; separates server faults from Finder faults
```

Write probe one-liners in bash or quote defensively: under zsh, `set -- $var`
does not word-split, and a broken loop made a healthy port read as closed once
already (2026-09-16). A false "closed" costs more than the quoting.

## Cleanup

```bash
ssh -t <alias> 'sudo systemctl disable --now smb.service && sudo rm /etc/samba/smb.conf'
ssh <alias> 'rm -f ~/smb.conf.staged ~/enable-file-share.sh'
```

The Samba password database (`/var/lib/samba/private/`) can stay; it holds
nothing but the share credentials.

## Provenance

First proven instance: omarchy machine `frank`, tailnet `100.92.62.101`, from
the owner's Mac, 2026-09-16 — staged over the `oi-omarchy` SSH alias, installed
with one sudo command, verified by Finder mount (`smb://frank`). The session
that earned this runbook also established the Finder-SFTP finding and the
loopback-binding failure mode recorded above.
