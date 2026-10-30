// Steps 2 and 3: Xbox Live user token, then an XSTS token for Minecraft.
import { requestJson, HttpError } from '../http.js';

const XBL_URL = 'https://user.auth.xboxlive.com/user/authenticate';
const XSTS_URL = 'https://xsts.auth.xboxlive.com/xsts/authorize';
export const MINECRAFT_RELYING_PARTY = 'rp://api.minecraftservices.com/';

const XSTS_ERRORS = {
  2148916227: 'This Xbox account is banned.',
  2148916233: 'This Microsoft account has no Xbox profile. Sign in at https://www.xbox.com once to create one.',
  2148916235: 'Xbox Live is not available in your region.',
  2148916236: 'This account needs adult verification (South Korea).',
  2148916237: 'This account needs adult verification (South Korea).',
  2148916238: 'This account belongs to a child and must be added to a family by an adult.',
};

export async function getXboxLiveToken(msaAccessToken) {
  const res = await requestJson(XBL_URL, {
    method: 'POST',
    body: {
      Properties: {
        AuthMethod: 'RPS',
        SiteName: 'user.auth.xboxlive.com',
        RpsTicket: `d=${msaAccessToken}`,
      },
      RelyingParty: 'http://auth.xboxlive.com',
      TokenType: 'JWT',
    },
  });
  return { token: res.Token, userHash: res.DisplayClaims.xui[0].uhs };
}

export async function getXstsToken(xblToken) {
  let res;
  try {
    res = await requestJson(XSTS_URL, {
      method: 'POST',
      body: {
        Properties: { SandboxId: 'RETAIL', UserTokens: [xblToken] },
        RelyingParty: MINECRAFT_RELYING_PARTY,
        TokenType: 'JWT',
      },
    });
  } catch (err) {
    if (err instanceof HttpError && err.status === 401) {
      const xerr = err.body?.XErr;
      throw new Error(XSTS_ERRORS[xerr] ?? `Xbox Live refused the sign-in (XErr ${xerr})`);
    }
    throw err;
  }
  return { token: res.Token, userHash: res.DisplayClaims.xui[0].uhs };
}
