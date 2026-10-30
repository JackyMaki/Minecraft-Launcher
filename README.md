# Minecraft Launcher

A Minecraft: Java Edition launcher with Microsoft account (MSA) login.

## Microsoft account login

The login flow lives in `src/auth/` and runs the standard chain:

1. Microsoft OAuth (device code flow, scope `XboxLive.signin offline_access`)
2. Xbox Live user token
3. XSTS token for `rp://api.minecraftservices.com/`
4. `login_with_xbox` on Minecraft services
5. Entitlement check (`/entitlements/mcstore`)
6. Profile lookup (`/minecraft/profile`)

The resulting account is saved to `account.json` in your OS config directory and
refreshed silently with the MSA refresh token when the Minecraft token expires.

### Getting a client ID

1. Register an app at https://portal.azure.com under **Microsoft Entra ID → App registrations**.
   Allow **personal Microsoft accounts** and, under **Authentication**, enable
   **Allow public client flows** (required for the device code flow).
2. Copy the **Application (client) ID**.
3. Submit it at https://aka.ms/mce-reviewappid so Mojang approves it for the Minecraft APIs.
   Until it is approved, the Xbox Live step will fail.

### Usage

```sh
cp .env.example .env         # put your client ID in MSA_CLIENT_ID
export $(cat .env | xargs)
npm run login                # prints a code to enter at microsoft.com/link
npm run logout
```

From code:

```js
import { getAccount, login, launchArgs } from './src/auth/index.js';

const clientId = process.env.MSA_CLIENT_ID;
const account = (await getAccount({ clientId }))
  ?? (await login({ clientId, onCode: (c) => console.log(c.message) }));

const { username, uuid, accessToken, userType } = launchArgs(account);
// pass these as --username --uuid --accessToken --userType to the game
```

### Tests

```sh
npm test
```
