import { TokensDialog } from "./Tokens.jsx";

// Fixed dialog header: what the dialog is for (`kind`), its `title`, and the close icon. The dialog body scrolls below it.
export const Header = ({ kind, title, onClose }) => (
  <header>
    <span class="tag">{kind}</span>
    <h2>{title}</h2>
    <button aria-label="Close" title="Close" onClick={onClose}>×</button>
  </header>
);

// Who is signed in, at the top right: name, personal tokens (developers only), log out.
export function UserBar({ me, onSignedOut }) {
  return (
    <div id="user">
      <span title={me.principal}>{me.name ?? me.principal}</span>
      {!me.admin && <TokensDialog />}
      <button onClick={onSignedOut}>Log out</button>
    </div>
  );
}
