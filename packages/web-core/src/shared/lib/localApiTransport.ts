const API_TOKEN_RELOAD_KEY = 'vk-api-token-reload-at';
const API_TOKEN_RELOAD_COOLDOWN_MS = 30_000;

function toAbsoluteWsUrl(pathOrUrl: string): string {
  if (/^wss?:\/\//i.test(pathOrUrl)) return pathOrUrl;
  if (/^https?:\/\//i.test(pathOrUrl)) return pathOrUrl.replace(/^http/i, 'ws');

  const protocol = window.location.protocol === 'https:' ? 'wss:' : 'ws:';
  const path = pathOrUrl.startsWith('/') ? pathOrUrl : `/${pathOrUrl}`;
  return `${protocol}//${window.location.host}${path}`;
}

function isSameOrigin(pathOrUrl: string): boolean {
  try {
    return (
      new URL(pathOrUrl, window.location.href).origin === window.location.origin
    );
  } catch {
    return false;
  }
}

// The server issues a new API token cookie with the HTML on every restart, so
// a 401 from our own origin means this page holds a stale cookie. Reloading
// fetches a fresh one; the cooldown prevents a reload loop if it keeps failing.
function reloadForFreshApiToken(): void {
  try {
    const last = Number(sessionStorage.getItem(API_TOKEN_RELOAD_KEY) ?? 0);
    if (Date.now() - last < API_TOKEN_RELOAD_COOLDOWN_MS) return;
    sessionStorage.setItem(API_TOKEN_RELOAD_KEY, String(Date.now()));
  } catch {
    return;
  }
  window.location.reload();
}

export async function makeLocalApiRequest(
  pathOrUrl: string,
  init: RequestInit = {}
): Promise<Response> {
  const response = await fetch(pathOrUrl, init);
  if (response.status === 401 && isSameOrigin(pathOrUrl)) {
    reloadForFreshApiToken();
  }
  return response;
}

export async function openLocalApiWebSocket(
  pathOrUrl: string
): Promise<WebSocket> {
  return new WebSocket(toAbsoluteWsUrl(pathOrUrl));
}
