// Persists the account on disk so the user does not have to log in every launch.
import { promises as fs } from 'node:fs';
import path from 'node:path';
import os from 'node:os';

export function defaultAccountPath() {
  const base = process.env.APPDATA
    || (process.platform === 'darwin' ? path.join(os.homedir(), 'Library', 'Application Support') : null)
    || process.env.XDG_CONFIG_HOME
    || path.join(os.homedir(), '.config');
  return path.join(base, 'minecraft-launcher', 'account.json');
}

export async function loadAccount(file = defaultAccountPath()) {
  try {
    return JSON.parse(await fs.readFile(file, 'utf8'));
  } catch (err) {
    if (err.code === 'ENOENT') return null;
    throw err;
  }
}

export async function saveAccount(account, file = defaultAccountPath()) {
  await fs.mkdir(path.dirname(file), { recursive: true });
  await fs.writeFile(file, JSON.stringify(account, null, 2), { mode: 0o600 });
}

export async function clearAccount(file = defaultAccountPath()) {
  await fs.rm(file, { force: true });
}
