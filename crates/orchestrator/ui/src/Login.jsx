import { useEffect, useState } from "preact/hooks";
import { AuthClient } from "@icp-sdk/auth/client";
import { bytesToBase64Url, deterministicEncode, signMessage } from "@ldclabs/ic-auth";
import { api, setToken } from "./api.js";

const EIGHT_HOURS_NS = 8n * 60n * 60n * 1_000_000_000n;

// Internet Identity (mainnet, https://id.ai) hands the browser a delegation; it signs the orchestrator's one-time
// challenge with it, and the orchestrator answers with a session token of its own.
async function signIn() {
  const identity = await new AuthClient().signIn({ maxTimeToLive: EIGHT_HOURS_NS });
  const { challenge } = await api("/auth/challenge");
  const envelope = bytesToBase64Url(deterministicEncode(await signMessage(identity, challenge)));
  const session = await api("/auth/login", { method: "POST", body: JSON.stringify({ envelope }) });
  setToken(session.token);
  return session;
}

export function Login({ onSignedIn }) {
  const [error, setError] = useState(null);
  const go = async e => {
    e.currentTarget.disabled = true;
    try { await signIn(); onSignedIn(); } catch (err) { setError(err.message); e.target.disabled = false; }
  };
  return (
    <main class="login">
      <h1>Software Factory</h1>
      <p>Sign in to see the board. Developers are identified by their Internet Identity principal and approved by the admin.</p>
      {error && <div class="error">{error}</div>}
      <button class="primary" onClick={go}>Sign in with Internet Identity</button>
    </main>
  );
}

// Signed in but not yet approved (or revoked): shows the principal for the admin to approve and polls until that happens.
export function Pending({ me, onChange, onSignedOut }) {
  useEffect(() => {
    const i = setInterval(() => api("/me").then(u => { if (u.status !== me.status) onChange(); }).catch(() => {}), 3000);
    return () => clearInterval(i);
  }, [me.status, onChange]);
  return (
    <main class="login">
      <h1>Software Factory</h1>
      {me.status === "pending"
        ? <p>Waiting for approval. Ask the admin to run:</p>
        : <p>This account was revoked. The admin can reinstate it with:</p>}
      <pre>factory user approve {me.principal} --name "…"</pre>
      <p class="principal">Your principal: <code>{me.principal}</code></p>
      <button onClick={onSignedOut}>Log out</button>
    </main>
  );
}
