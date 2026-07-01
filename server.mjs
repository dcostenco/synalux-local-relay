#!/usr/bin/env node
import 'dotenv/config';
import { createClient } from '@supabase/supabase-js';
import net from 'net';
import Bonjour from 'bonjour-service';
import { readFileSync, writeFileSync } from 'fs';
import { fileURLToPath } from 'url';
import { dirname, join } from 'path';
import { createHash } from 'crypto';
import { execSync } from 'child_process';

const __filename = fileURLToPath(import.meta.url);
const __dirname = dirname(__filename);
const VERSION = '2.0.0';

// Auto-update: check GitHub for newer server.mjs on startup
async function autoUpdate() {
  try {
    const updateUrl = 'https://raw.githubusercontent.com/dcostenco/synalux-local-relay/main/server.mjs';
    const res = await fetch(updateUrl);
    if (!res.ok) return false;
    const remote = await res.text();
    const local = readFileSync(__filename, 'utf-8');
    const localHash = createHash('sha256').update(local).digest('hex');
    const remoteHash = createHash('sha256').update(remote).digest('hex');
    if (localHash === remoteHash) {
      console.log(`[Auto-Update] v${VERSION} is up to date`);
      return false;
    }
    writeFileSync(__filename, remote, 'utf-8');
    console.log('[Auto-Update] Updated to latest version — restarting...');
    // If running under PM2, it auto-restarts on file change.
    // Otherwise, exit and let the user/wrapper restart.
    process.exit(0);
  } catch (err) {
    console.log(`[Auto-Update] Check failed (offline?): ${err.message}`);
    return false;
  }
}

await autoUpdate();

// RELAY_SECRET is the per-venue credential — used to authenticate with the DB queue.
// It's stored in .env as RELAY_HMAC_SECRET (legacy name) or RELAY_SECRET.
const RELAY_SECRET = process.env.RELAY_SECRET || process.env.RELAY_HMAC_SECRET || null;

process.on('uncaughtException', (err) => {
  console.error('[Synalux Local Relay] Uncaught Exception:', err);
});

process.on('unhandledRejection', (reason, promise) => {
  console.error('[Synalux Local Relay] Unhandled Rejection at:', promise, 'reason:', reason);
});

const SUPABASE_URL = process.env.SUPABASE_URL;
const SUPABASE_KEY = process.env.SUPABASE_KEY;
const RELAY_CHANNEL_ID = process.env.RELAY_CHANNEL_ID;

if (!SUPABASE_URL || !SUPABASE_KEY || !RELAY_CHANNEL_ID) {
  console.error("Missing required environment variables.");
  console.error("Required: SUPABASE_URL, SUPABASE_KEY, RELAY_CHANNEL_ID");
  process.exit(1);
}

const PRIVATE_HOST_RE = /^(localhost|127\.\d+\.\d+\.\d+|192\.168\.\d+\.\d+|10\.\d+\.\d+\.\d+|172\.(1[6-9]|2\d|3[01])\.\d+\.\d+)$/i;
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

const supabase = createClient(SUPABASE_URL, SUPABASE_KEY);

console.log(`[Synalux Local Relay] Starting daemon...`);
console.log(`[Synalux Local Relay] Venue: ${RELAY_CHANNEL_ID}`);
console.log(`[Synalux Local Relay] Mode: DB print queue (durable)`);

let printCount = 0;

async function processPrintJob(job) {
  try {
    if (job.printer_type === 'generic' || (!job.http_url && job.payload_base64)) {
      // TCP print
      const host = job.printer_ip;
      const port = 9100;

      if (!isAllowedHost(host)) {
        throw new Error(`Host ${host} not allowed`);
      }
      if (!ALLOWED_TCP_PORTS.has(port)) {
        throw new Error(`Port ${port} not allowed`);
      }

      const bytes = Buffer.from(job.payload_base64, 'base64');
      await new Promise((resolve, reject) => {
        const client = new net.Socket();
        client.setTimeout(3000);
        client.on('error', (err) => reject(new Error(`TCP error: ${err.message}`)));
        client.on('timeout', () => { client.destroy(); reject(new Error('TCP timeout')); });
        client.connect(port, host, () => {
          client.write(bytes, (err) => {
            if (err) reject(new Error(`Write error: ${err.message}`));
            else { client.end(); resolve(); }
          });
        });
      });

      console.log(`[tcp] Sent ${bytes.length} bytes to ${host}:${port}`);
    } else if (job.http_url) {
      // HTTP print (Star WebPRNT / Epson ePOS)
      if (!isAllowedUrl(job.http_url)) {
        throw new Error(`URL ${job.http_url} not allowed`);
      }

      const controller = new AbortController();
      const timeout = setTimeout(() => controller.abort(), 3000);
      const res = await fetch(job.http_url, {
        method: job.http_method || 'POST',
        headers: job.http_headers || {},
        body: job.http_body || '',
        signal: controller.signal,
      });
      clearTimeout(timeout);

      if (!res.ok) throw new Error(`HTTP ${res.status}`);
      console.log(`[http] Success: ${job.http_url} → ${res.status}`);
    }

    // Mark done
    await supabase.rpc('complete_print_job', { p_job_id: job.id, p_success: true, p_relay_secret: RELAY_SECRET, p_error: null });
    printCount++;
    return true;
  } catch (err) {
    console.error(`[print] Failed: ${err.message}`);
    await supabase.rpc('complete_print_job', {
      p_job_id: job.id,
      p_success: false,
      p_error: err.message,
      p_relay_secret: RELAY_SECRET,
    });
    return false;
  }
}

async function pollForJobs() {
  try {
    const { data: job, error } = await supabase.rpc('claim_print_job', {
      p_venue_id: RELAY_CHANNEL_ID,
      p_relay_secret: RELAY_SECRET,
    });

    if (error) {
      if (!error.message?.includes('no rows')) {
        console.error('[poll] Claim error:', error.message);
      }
      return false;
    }

    const row = Array.isArray(job) ? job[0] : job;
    if (!row?.id) return false;

    await processPrintJob(row);
    return true;
  } catch (err) {
    console.error('[poll] Error:', err.message);
    return false;
  }
}

// Main loop: poll + Realtime subscription for instant notification
async function main() {
  console.log(`[Synalux Local Relay] Starting print queue listener...`);

  // Subscribe to Realtime for instant job notifications
  const channel = supabase
    .channel(`print-jobs:${RELAY_CHANNEL_ID}`)
    .on('postgres_changes', {
      event: 'INSERT',
      schema: 'public',
      table: 'pos_print_jobs',
      filter: `venue_id=eq.${RELAY_CHANNEL_ID}`,
    }, async () => {
      // New job inserted — process immediately
      let hasMore = true;
      while (hasMore) {
        hasMore = await pollForJobs();
      }
    })
    .subscribe((status) => {
      if (status === 'SUBSCRIBED') {
        console.log('[Synalux Local Relay] Successfully subscribed and listening for print jobs!');
      } else if (status === 'CHANNEL_ERROR') {
        console.error('[Synalux Local Relay] Channel error — will reconnect');
      } else {
        console.log(`[Synalux Local Relay] Subscription status: ${status}`);
      }
    });

  // Also poll every 5s as a fallback (catches jobs missed during reconnects)
  setInterval(async () => {
    let hasMore = true;
    while (hasMore) {
      hasMore = await pollForJobs();
    }
  }, 5000);

  // Process any pending jobs from before we started
  let hasMore = true;
  while (hasMore) {
    hasMore = await pollForJobs();
  }
}

// Printer auto-discovery (Bonjour/mDNS)
const bonjour = new Bonjour();
console.log(`[Synalux Local Relay] Starting auto-discovery for local network printers...`);

bonjour.find({ type: 'printer' }, (service) => {
  console.log(`[Discovery] Found printer: ${service.name} at ${service.addresses[0]}:${service.port}`);
});

bonjour.find({ type: 'pdl-datastream' }, (service) => {
  console.log(`[Discovery] Found PDL printer: ${service.name} at ${service.addresses[0]}:${service.port}`);
});

// Graceful shutdown
async function shutdown(signal) {
  console.log(`[Synalux Local Relay] ${signal} received — shutting down...`);
  try {
    bonjour.destroy();
    await supabase.removeAllChannels();
  } catch (err) {
    console.error('[Synalux Local Relay] Shutdown error:', err.message);
  }
  process.exit(0);
}

process.on('SIGTERM', () => shutdown('SIGTERM'));
process.on('SIGINT', () => shutdown('SIGINT'));

main().catch((err) => {
  console.error('[Synalux Local Relay] Fatal:', err);
  process.exit(1);
});
