// Step 1: Microsoft account (MSA) OAuth via the device code flow.
// https://learn.microsoft.com/entra/identity-platform/v2-oauth2-device-code
import { requestJson, HttpError, sleep } from '../http.js';

const AUTHORITY = 'https://login.microsoftonline.com/consumers/oauth2/v2.0';
export const SCOPE = 'XboxLive.signin offline_access';

/**
 * Start the device code flow. Returns the code the user must enter and the
 * URL to enter it at, plus a `poll()` that resolves to the token set once
 * the user has signed in.
 */
export async function beginDeviceLogin(clientId) {
  if (!clientId) throw new Error('MSA client ID is required (set MSA_CLIENT_ID)');
  const dc = await requestJson(`${AUTHORITY}/devicecode`, {
    method: 'POST',
    form: { client_id: clientId, scope: SCOPE },
  });

  const deadline = Date.now() + dc.expires_in * 1000;
  let interval = (dc.interval ?? 5) * 1000;

  async function poll() {
    while (Date.now() < deadline) {
      await sleep(interval);
      try {
        const tok = await requestJson(`${AUTHORITY}/token`, {
          method: 'POST',
          form: {
            grant_type: 'urn:ietf:params:oauth:grant-type:device_code',
            client_id: clientId,
            device_code: dc.device_code,
          },
        });
        return normalize(tok);
      } catch (err) {
        if (!(err instanceof HttpError)) throw err;
        const code = err.body?.error;
        if (code === 'authorization_pending') continue;
        if (code === 'slow_down') { interval += 5000; continue; }
        if (code === 'authorization_declined') throw new Error('Sign-in was declined by the user');
        if (code === 'expired_token') break;
        throw new Error(`Microsoft sign-in failed: ${code ?? err.message}`);
      }
    }
    throw new Error('The device code expired before sign-in completed');
  }

  return {
    userCode: dc.user_code,
    verificationUri: dc.verification_uri,
    message: dc.message,
    expiresAt: deadline,
    poll,
  };
}

/** Exchange a refresh token for a new MSA access + refresh token. */
export async function refreshMsaToken(clientId, refreshToken) {
  const tok = await requestJson(`${AUTHORITY}/token`, {
    method: 'POST',
    form: {
      grant_type: 'refresh_token',
      client_id: clientId,
      refresh_token: refreshToken,
      scope: SCOPE,
    },
  });
  return normalize(tok);
}

function normalize(tok) {
  return {
    accessToken: tok.access_token,
    refreshToken: tok.refresh_token,
    expiresAt: Date.now() + tok.expires_in * 1000,
  };
}
