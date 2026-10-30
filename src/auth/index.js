// Orchestrates the full MSA -> Xbox Live -> XSTS -> Minecraft login chain.
import { beginDeviceLogin, refreshMsaToken } from './msa.js';
import { getXboxLiveToken, getXstsToken } from './xbox.js';
import { loginWithXbox, ownsMinecraft, getProfile } from './minecraft.js';
import { loadAccount, saveAccount, clearAccount } from './store.js';

export { beginDeviceLogin, refreshMsaToken, getXboxLiveToken, getXstsToken, loginWithXbox, ownsMinecraft, getProfile, loadAccount, saveAccount, clearAccount };

const SKEW_MS = 60_000;

/**
 * Turn an MSA token set into a full Minecraft session.
 * Returns the account object that gets persisted.
 */
export async function completeLogin(msa) {
  const xbl = await getXboxLiveToken(msa.accessToken);
  const xsts = await getXstsToken(xbl.token);
  const mc = await loginWithXbox(xsts.userHash, xsts.token);
  if (!(await ownsMinecraft(mc.accessToken))) {
    throw new Error('This Microsoft account does not own Minecraft: Java Edition, so Elo Launcher cannot launch it.');
  }
  const profile = await getProfile(mc.accessToken);
  return {
    type: 'msa',
    profile,
    minecraft: mc,
    msa,
    loggedInAt: new Date().toISOString(),
  };
}

/**
 * Interactive login. `onCode` receives { userCode, verificationUri, message }
 * so the UI can show the code to the user.
 */
export async function login({ clientId, onCode, storePath } = {}) {
  const dc = await beginDeviceLogin(clientId);
  onCode?.(dc);
  const msa = await dc.poll();
  const account = await completeLogin(msa);
  await saveAccount(account, storePath);
  return account;
}

/**
 * Return a usable account, refreshing tokens silently if needed.
 * Returns null if there is no stored account or the refresh token is dead.
 */
export async function getAccount({ clientId, storePath } = {}) {
  const account = await loadAccount(storePath);
  if (!account) return null;
  if (account.minecraft.expiresAt - SKEW_MS > Date.now()) return account;

  try {
    const msa = await refreshMsaToken(clientId, account.msa.refreshToken);
    const fresh = await completeLogin(msa);
    await saveAccount(fresh, storePath);
    return fresh;
  } catch {
    await clearAccount(storePath);
    return null;
  }
}

export async function logout({ storePath } = {}) {
  await clearAccount(storePath);
}

/** Arguments to pass to the game for this account. */
export function launchArgs(account) {
  return {
    username: account.profile.name,
    uuid: account.profile.id,
    accessToken: account.minecraft.accessToken,
    userType: 'msa',
  };
}
