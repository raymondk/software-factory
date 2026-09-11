import { createContext } from "preact";
import { useContext } from "preact/hooks";

// { refresh, select, showError }, provided by App.
export const Ctx = createContext(null);
export const useApp = () => useContext(Ctx);

// Disables the clicked/submitting button while the handler runs; errors go to the bar unless onError is given.
export const useBusy = onError => {
  const { showError } = useApp();
  return fn => async e => {
    const b = e.submitter ?? e.currentTarget;
    b.disabled = true;
    try { await fn(e); } catch (err) { (onError ?? showError)(err.message); } finally { b.disabled = false; }
  };
};
