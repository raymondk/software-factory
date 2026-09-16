// The session token from login lives in localStorage; requests send it as a bearer header. A 401 drops it and tells
// the app, which shows the sign-in screen.
const KEY = "factory.token";
export const getToken = () => { try { return localStorage.getItem(KEY); } catch { return null; } };
export const setToken = t => { try { t == null ? localStorage.removeItem(KEY) : localStorage.setItem(KEY, t); } catch {} };

export const api = (path, opts) => {
  const token = getToken();
  return fetch(path, { ...opts, headers: { "Content-Type": "application/json", ...(token ? { Authorization: "Bearer " + token } : {}) } }).then(async r => {
    if (r.status === 401) { setToken(null); dispatchEvent(new Event("factory:unauthorized")); }
    if (!r.ok) throw new Error((await r.json().catch(() => ({}))).error || r.statusText);
    return r.status === 204 ? null : r.json();
  });
};
export const patch = (id, body) => api("/tickets/" + id, { method: "PATCH", body: JSON.stringify(body) });
