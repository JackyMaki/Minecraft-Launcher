import { test, beforeEach, afterEach } from 'node:test';
import assert from 'node:assert/strict';
import os from 'node:os';
import path from 'node:path';
import { promises as fs } from 'node:fs';
import { login, getAccount, launchArgs, loadAccount } from '../src/auth/index.js';

const realFetch = globalThis.fetch;
let calls;
let tmp;

function json(body, status = 200) {
  return new Response(JSON.stringify(body), { status, headers: { 'Content-Type': 'application/json' } });
}

function fakeFetch(overrides = {}) {
  let tokenPolls = 0;
  return async (url, init) => {
    const u = String(url);
    const body = init?.body ? Object.fromEntries(new URLSearchParams(init.body)) : {};
    calls.push({ url: u, body: init?.body, headers: init?.headers });
    if (u.endsWith('/devicecode')) {
      return json({ device_code: 'dev', user_code: 'ABCD1234', verification_uri: 'https://microsoft.com/link', expires_in: 900, interval: 0 });
    }
    if (u.endsWith('/token')) {
      if (body.grant_type === 'refresh_token') {
        assert.equal(body.refresh_token, 'rt1');
        return json({ access_token: 'msa2', refresh_token: 'rt2', expires_in: 3600 });
      }
      tokenPolls += 1;
      if (tokenPolls === 1) return json({ error: 'authorization_pending' }, 400);
      return json({ access_token: 'msa1', refresh_token: 'rt1', expires_in: 3600 });
    }
    if (u.includes('user.auth.xboxlive.com')) {
      const b = JSON.parse(init.body);
      assert.match(b.Properties.RpsTicket, /^d=msa/);
      return json({ Token: 'xbl', DisplayClaims: { xui: [{ uhs: 'hash' }] } });
    }
    if (u.includes('xsts.auth.xboxlive.com')) {
      if (overrides.xstsError) return json({ XErr: overrides.xstsError }, 401);
      return json({ Token: 'xsts', DisplayClaims: { xui: [{ uhs: 'hash' }] } });
    }
    if (u.endsWith('/login_with_xbox')) {
      assert.equal(JSON.parse(init.body).identityToken, 'XBL3.0 x=hash;xsts');
      return json({ access_token: 'mc', expires_in: overrides.mcExpires ?? 86400 });
    }
    if (u.endsWith('/entitlements/mcstore')) {
      assert.equal(init.headers.Authorization, 'Bearer mc');
      return json({ items: overrides.noGame ? [] : [{ name: 'game_minecraft' }] });
    }
    if (u.endsWith('/minecraft/profile')) {
      return json({ id: 'uuid', name: 'Steve', skins: [], capes: [] });
    }
    throw new Error(`unexpected fetch ${u}`);
  };
}

beforeEach(async () => {
  calls = [];
  tmp = path.join(await fs.mkdtemp(path.join(os.tmpdir(), 'mcl-')), 'account.json');
});
afterEach(() => { globalThis.fetch = realFetch; });

test('full device-code login chain produces a persisted account', async () => {
  globalThis.fetch = fakeFetch();
  let shown;
  const account = await login({ clientId: 'cid', storePath: tmp, onCode: (c) => { shown = c; } });

  assert.equal(shown.userCode, 'ABCD1234');
  assert.equal(account.profile.name, 'Steve');
  assert.deepEqual(launchArgs(account), { username: 'Steve', uuid: 'uuid', accessToken: 'mc', userType: 'msa' });
  assert.deepEqual(await loadAccount(tmp), account);

  const order = calls.map((c) => new URL(c.url).hostname + new URL(c.url).pathname);
  assert.deepEqual(order, [
    'login.microsoftonline.com/consumers/oauth2/v2.0/devicecode',
    'login.microsoftonline.com/consumers/oauth2/v2.0/token',
    'login.microsoftonline.com/consumers/oauth2/v2.0/token',
    'user.auth.xboxlive.com/user/authenticate',
    'xsts.auth.xboxlive.com/xsts/authorize',
    'api.minecraftservices.com/authentication/login_with_xbox',
    'api.minecraftservices.com/entitlements/mcstore',
    'api.minecraftservices.com/minecraft/profile',
  ]);
});

test('expired minecraft token is refreshed via the MSA refresh token', async () => {
  globalThis.fetch = fakeFetch({ mcExpires: 0 });
  await login({ clientId: 'cid', storePath: tmp });
  calls = [];
  const account = await getAccount({ clientId: 'cid', storePath: tmp });
  assert.equal(account.msa.accessToken, 'msa2');
  assert.equal(account.msa.refreshToken, 'rt2');
  assert.ok(calls[0].url.endsWith('/token'));
});

test('account without the game is rejected', async () => {
  globalThis.fetch = fakeFetch({ noGame: true });
  await assert.rejects(login({ clientId: 'cid', storePath: tmp }), /does not own Minecraft/);
  assert.equal(await loadAccount(tmp), null);
});

test('XSTS errors are translated to readable messages', async () => {
  globalThis.fetch = fakeFetch({ xstsError: 2148916233 });
  await assert.rejects(login({ clientId: 'cid', storePath: tmp }), /no Xbox profile/);
});

test('getAccount returns null when nothing is stored', async () => {
  assert.equal(await getAccount({ clientId: 'cid', storePath: tmp }), null);
});
