// Steps 4 to 6: Minecraft services login, entitlement check, profile.
import { requestJson, HttpError } from '../http.js';

const MC_API = 'https://api.minecraftservices.com';

export async function loginWithXbox(userHash, xstsToken) {
  const res = await requestJson(`${MC_API}/authentication/login_with_xbox`, {
    method: 'POST',
    body: { identityToken: `XBL3.0 x=${userHash};${xstsToken}` },
  });
  return { accessToken: res.access_token, expiresAt: Date.now() + res.expires_in * 1000 };
}

export async function ownsMinecraft(mcAccessToken) {
  const res = await requestJson(`${MC_API}/entitlements/mcstore`, {
    headers: { Authorization: `Bearer ${mcAccessToken}` },
  });
  const names = new Set((res.items ?? []).map((i) => i.name));
  return names.has('game_minecraft') || names.has('product_minecraft');
}

export async function getProfile(mcAccessToken) {
  try {
    const res = await requestJson(`${MC_API}/minecraft/profile`, {
      headers: { Authorization: `Bearer ${mcAccessToken}` },
    });
    return { id: res.id, name: res.name, skins: res.skins ?? [], capes: res.capes ?? [] };
  } catch (err) {
    if (err instanceof HttpError && err.status === 404) {
      throw new Error('This account owns Minecraft but has no profile yet. Launch the official launcher once to pick a username.');
    }
    throw err;
  }
}
