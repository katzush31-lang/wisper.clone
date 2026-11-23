import { useCallback, useEffect, useState } from "react";
import { api, events, type AccountStatus } from "../shared/api";

/** Current account status, kept fresh by the "account-changed" event. null until known. */
export function useAccountStatus(): { status: AccountStatus | null; setStatus: (s: AccountStatus) => void } {
  const [status, setStatus] = useState<AccountStatus | null>(null);
  const set = useCallback((s: AccountStatus) => setStatus(s), []);
  useEffect(() => {
    let live = true;
    api.accountStatus().then((s) => live && setStatus(s)).catch(() => {});
    const un = events.accountChanged((s) => live && setStatus(s));
    return () => {
      live = false;
      un.then((f) => f()).catch(() => {});
    };
  }, []);
  return { status, setStatus: set };
}
