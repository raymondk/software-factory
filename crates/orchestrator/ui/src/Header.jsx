// Fixed dialog header: what the dialog is for (`kind`), its `title`, and the close icon. The dialog body scrolls below it.
export const Header = ({ kind, title, onClose }) => (
  <header>
    <span class="tag">{kind}</span>
    <h2>{title}</h2>
    <button aria-label="Close" title="Close" onClick={onClose}>×</button>
  </header>
);
