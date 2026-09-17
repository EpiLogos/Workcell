#!/usr/bin/env bash
# Stage a tailnet-gated Samba file share on a remote machine, reachable from a
# Mac Finder at smb://<host>. Runbook: docs/REMOTE-FILE-SHARING.md.
#
# Needs only a key-authorised SSH alias on this machine — no root here, and no
# root over SSH: the config and a one-shot installer are staged into the remote
# user's home directory, and the owner runs the single sudo command printed at
# the end. Proven live against omarchy (Arch Linux), 2026-09-16.
#
# Usage: scripts/stage-remote-file-share.sh <ssh-alias> [smb-user]
set -euo pipefail

if [ $# -lt 1 ]; then
    echo "usage: $0 <ssh-alias> [smb-user]" >&2
    exit 64
fi
alias_name="$1"

remote() { ssh -o BatchMode=yes -o ConnectTimeout=8 "$alias_name" "$@"; }

echo "== checking $alias_name over SSH (key auth only)"
remote_home="$(remote 'echo $HOME')"
smb_user="$(remote 'whoami')"
echo "   connected as ${smb_user}@${alias_name}, home ${remote_home}"

echo "== checking samba on the remote"
if ! remote 'pacman -Q samba smbclient' >/dev/null 2>&1; then
    echo "   samba is not installed on ${alias_name}. Install it first (needs sudo there):" >&2
    echo "     ssh -t ${alias_name} 'sudo pacman -S --needed samba smbclient'" >&2
    echo "   then re-run this script." >&2
    exit 1
fi
echo "   samba present"

echo "== staging ${remote_home}/smb.conf.staged"
remote 'cat > ~/smb.conf.staged' <<'EOF'
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
EOF

echo "== staging ${remote_home}/enable-file-share.sh"
remote 'cat > ~/enable-file-share.sh && chmod +x ~/enable-file-share.sh' <<EOF
#!/bin/sh
# One-shot installer staged by stage-remote-file-share.sh. Needs root.
set -e
install -o root -g root -m 0644 ${remote_home}/smb.conf.staged /etc/samba/smb.conf
echo "== Choose a password for SMB access as user '${smb_user}' (independent of the login password) =="
smbpasswd -a ${smb_user}
systemctl enable --now smb.service
echo "== smb.service =="
systemctl --no-pager status smb.service | head -4 || true
echo "== port 445 =="
ss -tln | grep ':445 ' || echo "445 NOT LISTENING"
EOF

echo
echo "== staged. the owner runs this one command and types their sudo password:"
echo
echo "     ssh -t ${alias_name} 'sh ~/enable-file-share.sh'"
echo
echo "== expect: smb.service active (running) and a 0.0.0.0:445 (or *:445) LISTEN line."
echo "   127.0.0.1:445 alone means loopback-only — see docs/REMOTE-FILE-SHARING.md."
echo "   then Finder: ⌘K → smb://${alias_name} (or the machine's tailnet name),"
echo "   user ${smb_user}, the SMB password chosen above."
