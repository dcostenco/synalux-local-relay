#!/usr/bin/env node
import 'dotenv/config';
import crypto from 'crypto';
import { createClient } from '@supabase/supabase-js';
import net from 'net';
import Bonjour from 'bonjour-service';

process.on('uncaughtException', (err) => {
  console.error('[Synalux Local Relay] Uncaught Exception:', err);
});

process.on('unhandledRejection', (reason, promise) => {
  console.error('[Synalux Local Relay] Unhandled Rejection at:', promise, 'reason:', reason);
});

const SUPABASE_URL = process.env.SUPABASE_URL;
const SUPABASE_KEY = process.env.SUPABASE_KEY;
const RELAY_CHANNEL_ID = process.env.RELAY_CHANNEL_ID;
const RELAY_HMAC_SECRET = process.env.RELAY_HMAC_SECRET;

if (!SUPABASE_URL || !SUPABASE_KEY || !RELAY_CHANNEL_ID) {
  console.error("Missing required environment variables. Please check your .env file.");
  console.error("Required: SUPABASE_URL, SUPABASE_KEY, RELAY_CHANNEL_ID, RELAY_HMAC_SECRET");
  process.exit(1);
}
if (!RELAY_HMAC_SECRET) {
  console.error("FATAL: RELAY_HMAC_SECRET is required. Generate with: openssl rand -hex 32");
  console.error("Add RELAY_HMAC_SECRET to your .env and set the same value in Vercel/POS env.");
  process.exit(1);
}

// ---------------------------------------------------------------------------
// Security helpers
// ---------------------------------------------------------------------------

function deepSortedStringify(obj) {
  if (obj === null || typeof obj !== 'object') return JSON.stringify(obj);
  if (Array.isArray(obj)) return '[' + obj.map(deepSortedStringify).join(',') + ']';
  const sorted = Object.keys(obj).sort();
  return '{' + sorted.map(k => JSON.stringify(k) + ':' + deepSortedStringify(obj[k])).join(',') + '}';
}

function verifyPayloadHmac(data) {
  const { _sig, ...rest } = data;
  if (!_sig || typeof _sig !== 'string') return false;
  try {
    const canonical = deepSortedStringify(rest);
    const expected = crypto.createHmac('sha256', RELAY_HMAC_SECRET)
      .update(canonical)
      .digest('hex');
    const expectedBuf = Buffer.from(expected, 'utf8');
    const receivedBuf = Buffer.from(_sig, 'utf8');
    if (expectedBuf.length !== receivedBuf.length) return false;
    return crypto.timingSafeEqual(expectedBuf, receivedBuf);
  } catch {
    return false;
  }
}

// Only allow RFC1918 / loopback hosts — printer-only use case.
const PRIVATE_HOST_RE = /^(localhost|127\.\d+\.\d+\.\d+|192\.168\.\d+\.\d+|10\.\d+\.\d+\.\d+|172\.(1[6-9]|2\d|3[01])\.\d+\.\d+)$/i;

// Only allow common ESC/POS, Star, and LPD printer ports.
const ALLOWED_TCP_PORTS = new Set([9100, 6101, 515, 9101]);

function isAllowedHost(host) {
  return typeof host === 'string' && PRIVATE_HOST_RE.test(host);
}

function isAllowedUrl(urlStr) {
  try {
    const u = new URL(urlStr);
    return (u.protocol === 'http:') && isAllowedHost(u.hostname);
  } catch {
    return false;
  }
}

// ---------------------------------------------------------------------------
// Supabase channel
// ---------------------------------------------------------------------------

const supabase = createClient(SUPABASE_URL, SUPABASE_KEY);
const channelName = `local-relay:${RELAY_CHANNEL_ID}`;

console.log(`[Synalux Local Relay] Starting daemon...`);
console.log(`[Synalux Local Relay] Connecting to Supabase channel: ${channelName}`);

const channel = supabase.channel(channelName);

channel
  .on('broadcast', { event: 'tcp-request' }, async (payload) => {
    const data = payload.payload;

    if (!verifyPayloadHmac(data)) {
      console.error('[tcp-request] HMAC verification failed — ignoring message');
      return;
    }

    if (!isAllowedHost(data.host)) {
      console.error(`[tcp-request] Host not allowed: ${data.host}`);
      return;
    }

    const port = Number(data.port);
    if (!Number.isInteger(port) || !ALLOWED_TCP_PORTS.has(port)) {
      console.error(`[tcp-request] Port not allowed: ${data.port}`);
      return;
    }

    if (typeof data.bytesBase64 !== 'string' || data.bytesBase64.length > 262144) {
      console.error('[tcp-request] Payload too large or invalid');
      return;
    }

    console.log(`[tcp-request] Printing to ${data.host}:${port}`);

    try {
      const bytes = Buffer.from(data.bytesBase64, 'base64');
      const client = new net.Socket();
      client.setTimeout(3000);

      client.on('error', (err) => {
        console.error(`[tcp-request] Error connecting to ${data.host}: ${err.message}`);
      });

      client.on('timeout', () => {
        console.error(`[tcp-request] Timeout connecting to ${data.host}`);
        client.destroy();
      });

      client.connect(port, data.host, () => {
        client.write(bytes, (err) => {
          if (err) {
            console.error(`[tcp-request] Error writing to ${data.host}`);
          } else {
            console.log(`[tcp-request] Successfully sent ${bytes.length} bytes to ${data.host}`);
          }
          client.end();
        });
      });
    } catch (err) {
      console.error(`[tcp-request] Exception: ${err.message}`);
    }
  })
  .on('broadcast', { event: 'http-request' }, async (payload) => {
    const data = payload.payload;

    if (!verifyPayloadHmac(data)) {
      console.error('[http-request] HMAC verification failed — ignoring message');
      return;
    }

    if (!isAllowedUrl(data.url)) {
      console.error(`[http-request] URL not allowed: ${data.url}`);
      return;
    }

    if (typeof data.body === 'string' && data.body.length > 524288) {
      console.error('[http-request] Body too large');
      return;
    }

    console.log(`[http-request] Sending to ${data.url}`);

    try {
      const controller = new AbortController();
      const timeout = setTimeout(() => controller.abort(), 3000);

      const res = await fetch(data.url, {
        method: data.method || 'POST',
        headers: data.headers || {},
        body: data.body,
        signal: controller.signal,
      });
      clearTimeout(timeout);

      if (res.ok) {
        console.log(`[http-request] Success: ${res.status}`);
      } else {
        console.error(`[http-request] Failed with status ${res.status}`);
      }
    } catch (err) {
      console.error(`[http-request] Exception: ${err.message}`);
    }
  })
  .subscribe((status) => {
    if (status === 'SUBSCRIBED') {
      console.log(`[Synalux Local Relay] Successfully subscribed and listening for events!`);
    } else {
      console.log(`[Synalux Local Relay] Subscription status: ${status}`);
    }
  });

// ---------------------------------------------------------------------------
// Printer auto-discovery (Bonjour/mDNS)
// ---------------------------------------------------------------------------

const bonjour = new Bonjour();
console.log(`[Synalux Local Relay] Starting auto-discovery for local network printers...`);

bonjour.find({ type: 'printer' }, (service) => {
  console.log(`[Discovery] Found printer: ${service.name} at ${service.addresses[0]}:${service.port}`);
  channel.send({
    type: 'broadcast',
    event: 'printer-discovered',
    payload: {
      name: service.name,
      address: service.addresses[0],
      port: service.port,
      fqdn: service.fqdn,
    },
  }).catch(err => console.error(`[Discovery] Failed to broadcast printer to cloud:`, err));
});

bonjour.find({ type: 'pdl-datastream' }, (service) => {
  console.log(`[Discovery] Found PDL printer: ${service.name} at ${service.addresses[0]}:${service.port}`);
  channel.send({
    type: 'broadcast',
    event: 'printer-discovered',
    payload: {
      name: service.name,
      address: service.addresses[0],
      port: service.port,
      fqdn: service.fqdn,
      type: 'pdl',
    },
  }).catch(err => console.error(`[Discovery] Failed to broadcast PDL printer:`, err));
});

// ---------------------------------------------------------------------------
// Graceful shutdown
// ---------------------------------------------------------------------------

async function shutdown(signal) {
  console.log(`[Synalux Local Relay] ${signal} received — shutting down...`);
  try {
    bonjour.destroy();
    await supabase.removeChannel(channel);
  } catch (err) {
    console.error('[Synalux Local Relay] Shutdown error:', err.message);
  }
  process.exit(0);
}

process.on('SIGTERM', () => shutdown('SIGTERM'));
process.on('SIGINT', () => shutdown('SIGINT'));
