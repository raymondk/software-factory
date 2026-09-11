import { createContext } from "preact";
import { useContext } from "preact/hooks";

// { refresh, select, showError }, provided by App.
export const Ctx = createContext(null);
export const useApp = () => useContext(Ctx);

// Disables the clicked/submitting button while the handler runs; errors go to the bar.
export const useBusy = () => {
  const { showError } = useApp();
  return fn => async e => {
    const b = e.submitter ?? e.currentTarget;
    b.disabled = true;
    try { await fn(e); } catch (err) { showError(err.message); } finally { b.disabled = false; }
  };
};
