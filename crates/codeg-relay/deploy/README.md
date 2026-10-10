# codeg-relay deployment kit

See [README.zh-CN.md](README.zh-CN.md) for the full CLI, installation, isolation
checks and rollback guide. This kit serves one active home codeg agent over a
TLS-terminated, multiplexed WSS tunnel. Relay administrators can see HTTP content
and the web token; use a trusted VPS.

Download the Linux x64 or arm64 tarball and matching `.sha256` from
https://github.com/PanAndDuck/codeg-fork/releases. Verify with
`sha256sum -c codeg-relay-linux-<arch>.tar.gz.sha256`, then extract it and enter
the extracted directory:

```bash
sudo bash deploy/install.sh --binary ./codeg-relay \
  --port 18443 --public-host relay.example.com
```

Replace `relay.example.com` with your server's DNS name or IP. Allow that TCP
port in the provider firewall; optionally pass `--open-firewall` to manage UFW.
The installer needs systemd, useradd, flock and ss, and does not install packages.
Keep the generated agent key and certificate fingerprint private. Configure
Settings → Web service → Remote relay in home codeg, then pair iOS using codeg's
QR. The agent key is different from the web token used by iOS.

The install owns `/opt/codeg-relay`, `/etc/codeg-relay`, `/var/lib/codeg-relay`,
its dedicated user and unit. Verify existing services remain unaffected before
and after deployment. `sudo bash deploy/uninstall.sh` removes only managed
resources and its own firewall rule; back up the key/certificate first if you
want to preserve the relay identity.
