# Synalux Local Relay

A lightweight background service that bridges the Vercel-hosted [Synalux POS](https://pos.synalux.ai) to local network devices (ESC/POS printers, cash drawers) at your venue.

Since Vercel cannot reach `192.168.x.x` addresses, this relay connects to a Supabase Realtime channel and forwards print and device commands to your local LAN.

## Download

Download the relay from inside Synalux POS: **Settings → Printers & KDS → Cloud Print Relay**
([pos.synalux.ai/pos/settings/printers](https://pos.synalux.ai/pos/settings/printers)).
It offers the installer for the release your POS supports:

- **macOS (Apple Silicon)**: `.dmg`
- **Windows x64**: `.msi`

[All releases](https://github.com/dcostenco/synalux-local-relay/releases)

There is no installer for Intel Macs or Linux.

## Quick Start

1. In Synalux POS, open **Settings → Printers & KDS → Cloud Print Relay** and install the relay app for your platform
2. Click **Add relay computer**
3. Click **Open Relay App** and allow the browser prompt
4. Confirm the relay dialog, then check that the tray shows **Status: Connected**
5. Click **Test Print** on any printer

The desktop relay needs no command line or configuration file: it claims the one-time setup link from POS. It runs in the system tray and starts automatically on boot.

## Building from Source

The Node relay (`server.mjs`) is the legacy command-line version, kept for existing installs. It does not pair through POS; it reads its settings from `.env`.

```bash
git clone https://github.com/dcostenco/synalux-local-relay.git
cd synalux-local-relay
npm install
cp .env.example .env   # then fill in the values below
node server.mjs
```

### Configuration (`.env`, Node relay only)
```env
SUPABASE_URL=https://your-project.supabase.co
SUPABASE_KEY=your-supabase-anon-or-service-key
RELAY_CHANNEL_ID=your-venue-id-here
RELAY_SECRET=your-venue-relay-secret
```

`RELAY_SECRET` is the relay secret Synalux POS issued for your venue (the older name `RELAY_HMAC_SECRET` is still read). The print queue checks it against the venue's stored secret, so a value you generate yourself is rejected.

### Production Deployment (PM2)

```bash
npm install -g pm2
pm2 start ecosystem.config.cjs
pm2 startup
pm2 save
```

## How It Works

1. Synalux POS broadcasts a print/device request to Supabase over the `local-relay:{venueId}` channel
2. This relay receives the payload and forwards it to the local device (e.g. `192.168.1.50:9100`)
3. All payloads are HMAC-signed and verified; only RFC1918 addresses and printer ports are allowed

## Documentation

Full printer setup guide: [Synalux POS Docs — Printers & Cash Drawer](https://github.com/dcostenco/synalux-docs/blob/main/docs_source_en/pos.md#printers--cash-drawer)

## License

[MIT](LICENSE)
