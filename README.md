# Synalux Local Relay

A lightweight background service that bridges the Vercel-hosted [Synalux POS](https://pos.synalux.ai) to local network devices (ESC/POS printers, cash drawers) at your venue.

Since Vercel cannot reach `192.168.x.x` addresses, this relay connects to a Supabase Realtime channel and forwards print and device commands to your local LAN.

## Download

Download the relay from inside Synalux POS: **Settings → Printers → Cloud Print Relay**
([pos.synalux.ai/pos/settings/printers](https://pos.synalux.ai/pos/settings/printers)).
It offers the installer for the release your POS supports:

- **macOS (Apple Silicon)**: `.dmg`
- **Windows x64**: `.msi`

[All releases](https://github.com/dcostenco/synalux-local-relay/releases)

## Quick Start

1. Download the relay from Synalux POS (Settings → Printers → Cloud Print Relay)
2. Copy `.env.example` to `.env` and fill in your credentials
3. Run the binary

### Configuration (`.env`)
```env
SUPABASE_URL=https://your-project.supabase.co
SUPABASE_KEY=your-supabase-anon-or-service-key
RELAY_CHANNEL_ID=your-venue-id-here
RELAY_HMAC_SECRET=your-hmac-secret-here
```

Generate your HMAC secret: `openssl rand -hex 32`

## Building from Source

```bash
git clone https://github.com/dcostenco/synalux-local-relay.git
cd synalux-local-relay
npm install
node server.mjs
```

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
