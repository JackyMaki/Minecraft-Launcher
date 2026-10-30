#!/usr/bin/env node
import { login, getAccount, logout } from '../src/auth/index.js';

const clientId = process.env.MSA_CLIENT_ID;
const args = process.argv.slice(2);

try {
  if (args.includes('--logout')) {
    await logout();
    console.log('Logged out.');
    process.exit(0);
  }

  let account = await getAccount({ clientId });
  if (account) {
    console.log(`Already logged in as ${account.profile.name} (${account.profile.id})`);
    process.exit(0);
  }

  account = await login({
    clientId,
    onCode: ({ userCode, verificationUri }) => {
      console.log(`\nOpen ${verificationUri} and enter the code: ${userCode}\n`);
      console.log('Waiting for you to sign in...');
    },
  });
  console.log(`Logged in as ${account.profile.name} (${account.profile.id})`);
} catch (err) {
  console.error(`Login failed: ${err.message}`);
  if (err.body) console.error(JSON.stringify(err.body, null, 2));
  process.exit(1);
}
