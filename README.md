# Synalux Local Relay Daemon

The Synalux Local Relay is a lightweight background service that bridges the Vercel-hosted Synalux cloud applications to local network devices (like ESC/POS printers and cash drawers) at your venue. 

Since Vercel cannot reach `192.168.x.x` IP addresses, this relay securely connects to a Supabase Realtime channel and listens for broadcast events from the POS. When an event is received, the relay executes it over the local LAN.

## Installation

### Prerequisites
- Node.js v18 or later
- A machine on the venue's local network (e.g. Mac Mini, Windows POS machine, Raspberry Pi)
- Internet access to connect to Supabase

### Setup
1. Clone or copy this directory to the local machine.
2. Run `npm install` to install dependencies.
3. Copy `.env.example` to `.env` and fill in your credentials.

### Configuration (`.env`)
```env
SUPABASE_URL=https://your-project.supabase.co
SUPABASE_KEY=your-supabase-anon-or-service-key
RELAY_CHANNEL_ID=your-venue-id-here
```

## Running the Relay

### Development/Testing Mode
You can start the relay manually:
```bash
node server.mjs
```
*Note: Make sure your `.env` is properly configured.*

### Production Deployment (PM2)
For production environments, we strongly recommend using PM2 to manage the relay. PM2 ensures the daemon automatically restarts if it crashes, and starts automatically when the machine boots up.

1. Install PM2 globally:
   ```bash
   npm install -g pm2
   ```

2. Start the relay:
   ```bash
   pm2 start ecosystem.config.cjs
   ```

3. Configure PM2 to start on system boot:
   ```bash
   pm2 startup
   pm2 save
   ```

To view logs:
```bash
pm2 logs synalux-local-relay
```

## How It Works
1. The Synalux Next.js POS app determines if a local IP request needs to be relayed.
2. It broadcasts the request (TCP socket or HTTP fetch) to Supabase over the `local-relay:{venueId}` channel.
3. This local daemon receives the payload instantly and makes the direct connection to the printer (e.g., `192.168.1.50:9100` or port 80).
