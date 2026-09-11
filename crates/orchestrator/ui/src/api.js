// The orchestrator fills the token in when serving the built page; the Vite dev server proxies to it and needs VITE_TOKEN.
const TOKEN = import.meta.env.DEV ? import.meta.env.VITE_TOKEN : document.querySelector("meta[name=token]").content;
const headers = { Authorization: "Bearer " + TOKEN, "Content-Type": "application/json" };

export const api = (path, opts) => fetch(path, { headers, ...opts }).then(async r => {
  if (!r.ok) throw new Error((await r.json()).error || r.statusText);
  return r.json();
});
export const patch = (id, body) => api("/tickets/" + id, { method: "PATCH", body: JSON.stringify(body) });
