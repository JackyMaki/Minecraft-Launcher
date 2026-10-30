export class HttpError extends Error {
  constructor(message, { status, url, body }) {
    super(message);
    this.name = 'HttpError';
    this.status = status;
    this.url = url;
    this.body = body;
  }
}

/** POST/GET JSON and parse the JSON response, throwing HttpError on non-2xx. */
export async function requestJson(url, { method = 'GET', headers = {}, body, form } = {}) {
  const init = { method, headers: { Accept: 'application/json', ...headers } };
  if (form) {
    init.headers['Content-Type'] = 'application/x-www-form-urlencoded';
    init.body = new URLSearchParams(form).toString();
  } else if (body !== undefined) {
    init.headers['Content-Type'] = 'application/json';
    init.body = JSON.stringify(body);
  }
  const res = await fetch(url, init);
  const text = await res.text();
  let data;
  try { data = text ? JSON.parse(text) : {}; } catch { data = { raw: text }; }
  if (!res.ok) {
    throw new HttpError(`${method} ${url} failed with HTTP ${res.status}`, { status: res.status, url, body: data });
  }
  return data;
}

export const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
