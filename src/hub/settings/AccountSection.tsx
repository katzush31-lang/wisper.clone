import { useCallback, useEffect, useState } from "react";
import { api, events, type AccountDevice, type AccountStatus, type KeyStorage } from "../../shared/api";
import { KEY_STORAGE_DESCRIPTIONS, accountState, syncLine, timeAgo, whoLabel } from "../../shared/account";
import { ConfirmDialog, Segmented, SettingGroup, SettingRow, errorText, useToast } from "../components";
import { useAccountStatus } from "../useAccount";

type Confirm = null | "restore" | "keys-local" | "delete" | { signOutDevice: AccountDevice };

export default function AccountSection({ onDone }: { onDone?: () => void }) {
  const toast = useToast();
  const { status, setStatus } = useAccountStatus();
  const [confirm, setConfirm] = useState<Confirm>(null);
  const [busy, setBusy] = useState(false);
  const [signingIn, setSigningIn] = useState(false);
  const [signInMsg, setSignInMsg] = useState<string | null>(null);
  const [syncing, setSyncing] = useState(false);
  const [devices, setDevices] = useState<AccountDevice[]>([]);
  const [now, setNow] = useState(Date.now());

  // keep "Synced 2 minutes ago" ticking
  useEffect(() => {
    const t = window.setInterval(() => setNow(Date.now()), 30_000);
    return () => window.clearInterval(t);
  }, []);

  const state = accountState(status);
  const signedIn = state === "signed-in";
  const lastSync = status?.lastSync;

  const loadDevices = useCallback(() => {
    api.accountDevices().then(setDevices).catch(() => {});
  }, []);
  useEffect(() => {
    if (signedIn) loadDevices();
    else setDevices([]);
  }, [signedIn, lastSync, loadDevices]);

  // a sign-in finishing elsewhere (event) ends the waiting state
  useEffect(() => {
    const un = events.accountChanged((s) => {
      if (s.signedIn) {
        setSigningIn(false);
        setSignInMsg(null);
      }
    });
    return () => {
      un.then((f) => f()).catch(() => {});
    };
  }, []);

  const signIn = async () => {
    setSigningIn(true);
    setSignInMsg(null);
    try {
      const s = await api.accountSignIn();
      // ignore the result if the person pressed Cancel in the meantime
      setStatus(s);
      if (s.signedIn) toast("Signed in");
    } catch (e) {
      setSignInMsg(errorText(e));
    } finally {
      setSigningIn(false);
    }
  };

  const act = async (fn: () => Promise<AccountStatus | void>, okMsg?: string) => {
    setBusy(true);
    try {
      const s = await fn();
      if (s) setStatus(s);
      if (okMsg) toast(okMsg);
    } catch (e) {
      toast(errorText(e), "error");
    } finally {
      setBusy(false);
    }
  };

  const syncNow = async () => {
    setSyncing(true);
    try {
      setStatus(await api.accountSyncNow());
    } catch (e) {
      toast(errorText(e), "error");
    } finally {
      setSyncing(false);
    }
  };

  const setKeyStorage = (storage: KeyStorage) => {
    if (!status) return;
    // First choice on this computer: nothing to remove yet, so no confirm needed.
    if (status.keyStorageChosen === false) {
      void act(
        () => api.accountSetKeyStorage(storage),
        storage === "cloud" ? "Your keys are now synced through your account" : "Your keys stay on this computer only",
      );
      return;
    }
    if (storage === status.keyStorage) return;
    if (storage === "local") setConfirm("keys-local");
    else void act(() => api.accountSetKeyStorage("cloud"), "Your keys are now synced through your account");
  };

  const runBackup = async (fn: () => Promise<string>, prefix: string) => {
    setBusy(true);
    try {
      const path = await fn();
      toast(path ? `${prefix} ${path}` : prefix);
      onDone?.();
    } catch (e) {
      toast(errorText(e), "error");
    } finally {
      setBusy(false);
    }
  };

  const sync = status ? syncLine(status, now) : null;

  return (
    <>
      {state === "unavailable" && (
        <p className="section-intro">Online accounts aren't available in this version of Wisper. Everything lives on this PC, and a backup file lets you move it to another PC or keep a safe copy.</p>
      )}

      {state === "signed-out" && (
        <>
          <div className="section-intro account-intro">
            <p>
              <strong>Wisper works fully without an account.</strong> If you sign in, you can use Wisper on several computers with the same API keys, settings, dictionary, snippets and word counts.
            </p>
            <p>Your recordings and history stay on each computer. They are never uploaded.</p>
          </div>
          <SettingGroup>
            <SettingRow
              title="Wisper account"
              description={
                signingIn
                  ? "Finish signing in in your browser…"
                  : signInMsg ?? "Sign in with your Google account. It opens in your web browser and takes a few seconds."
              }
            >
              {signingIn ? (
                <div className="account-actions">
                  <span className="account-waiting" role="status">
                    <span className="spinner" aria-hidden /> Waiting…
                  </span>
                  <button className="btn btn-soft" onClick={() => { setSigningIn(false); setSignInMsg("Sign-in cancelled. You can try again whenever you like."); }}>
                    Cancel
                  </button>
                </div>
              ) : (
                <button className="btn btn-dark" onClick={() => void signIn()}>
                  Sign in with Google
                </button>
              )}
            </SettingRow>
          </SettingGroup>
        </>
      )}

      {signedIn && status && status.keyStorageChosen === false && (
        <div className="card account-choose" role="region" aria-label="Choose where to keep your API keys">
          <strong>One last step: where should your API keys be kept?</strong>
          <p className="muted">Nothing moves until you choose. You can change this later.</p>
          <div className="account-choose-btns">
            <button className="btn btn-dark" disabled={busy} onClick={() => setKeyStorage("cloud")}>
              In my account (synced)
            </button>
            <button className="btn btn-soft" disabled={busy} onClick={() => setKeyStorage("local")}>
              This computer only
            </button>
          </div>
          <p className="muted account-choose-note">
            {KEY_STORAGE_DESCRIPTIONS.cloud} {KEY_STORAGE_DESCRIPTIONS.local}
          </p>
        </div>
      )}

      {signedIn && status && (
        <>
          <SettingGroup title="Your account">
            <SettingRow title={status.name || "Signed in"} description={status.email ?? "You are signed in."}>
              <button className="btn btn-soft" disabled={busy} onClick={() => void act(() => api.accountSignOut(), "Signed out")}>
                Sign out
              </button>
            </SettingRow>
            <SettingRow title="Sync" description="Keeps your settings, dictionary, snippets and word counts the same on all your computers. It happens by itself in the background.">
              <div className="account-actions">
                {sync && (
                  <span className={`account-sync ${sync.tone}`} role="status">
                    {sync.text}
                  </span>
                )}
                <button className="btn btn-soft" disabled={syncing} onClick={() => void syncNow()}>
                  {syncing ? "Syncing…" : "Sync now"}
                </button>
              </div>
            </SettingRow>
            <SettingRow title="Where to keep your API keys" description={KEY_STORAGE_DESCRIPTIONS[status.keyStorage]} stacked>
              <Segmented<KeyStorage>
                value={status.keyStorage}
                options={[
                  { value: "local", label: "This computer only" },
                  { value: "cloud", label: "Your account (synced)" },
                ]}
                onChange={setKeyStorage}
              />
            </SettingRow>
          </SettingGroup>

          <SettingGroup title="Your computers">
            {devices.length === 0 ? (
              <div className="setting-row">
                <div className="setting-desc">Loading your computers…</div>
              </div>
            ) : (
              devices.map((d) => (
                <SettingRow
                  key={d.id}
                  title={d.name}
                  description={d.current ? "The computer you are using now." : `Last used ${timeAgo(d.lastSeen, now)}`}
                >
                  {d.current ? (
                    <span className="badge badge-ok">This computer</span>
                  ) : (
                    <button className="btn btn-soft" disabled={busy} onClick={() => setConfirm({ signOutDevice: d })}>
                      Sign out
                    </button>
                  )}
                </SettingRow>
              ))
            )}
          </SettingGroup>
        </>
      )}

      <SettingGroup title="Backup">
        <SettingRow title="Export backup" description="Saves your settings, dictionary, snippets and modes into one file you choose where to keep.">
          <button className="btn btn-soft" disabled={busy} onClick={() => void runBackup(api.exportBackup, "Backup saved to")}>
            Export backup
          </button>
        </SettingRow>
        <SettingRow title="Restore from backup" description="Loads a backup file you saved earlier. This replaces what you have now.">
          <button className="btn btn-soft" disabled={busy} onClick={() => setConfirm("restore")}>
            Restore from backup
          </button>
        </SettingRow>
      </SettingGroup>

      {signedIn && (
        <SettingGroup title="Danger zone">
          <SettingRow title="Delete account" description="Permanently deletes your synced data from the cloud and signs you out everywhere. Wisper keeps working on this PC with what is saved here.">
            <button className="btn btn-danger" disabled={busy} onClick={() => setConfirm("delete")}>
              Delete account…
            </button>
          </SettingRow>
        </SettingGroup>
      )}

      <ConfirmDialog
        open={confirm === "restore"}
        title="Restore from backup?"
        message="This replaces your current settings, dictionary, snippets and modes with the ones in the backup file. This can't be undone, so export a backup first if you might want today's version."
        confirmLabel="Choose file…"
        onCancel={() => setConfirm(null)}
        onConfirm={() => {
          setConfirm(null);
          void runBackup(api.importBackup, "Restored");
        }}
      />
      <ConfirmDialog
        open={confirm === "keys-local"}
        title="Keep your keys on this computer only?"
        message="Your API keys will be removed from the cloud, so your other computers will need their own keys. The keys stay on this PC and keep working here."
        confirmLabel="Remove from cloud"
        onCancel={() => setConfirm(null)}
        onConfirm={() => {
          setConfirm(null);
          void act(() => api.accountSetKeyStorage("local"), "Your keys are now kept on this computer only");
        }}
      />
      <ConfirmDialog
        open={typeof confirm === "object" && confirm !== null}
        title="Sign out that computer?"
        message={`${typeof confirm === "object" && confirm ? confirm.signOutDevice.name : "That computer"} will be signed out of your account and stop syncing. Nothing is deleted from it, and you can sign in there again any time.`}
        confirmLabel="Sign out"
        onCancel={() => setConfirm(null)}
        onConfirm={() => {
          const d = typeof confirm === "object" && confirm ? confirm.signOutDevice : null;
          setConfirm(null);
          if (d) void act(async () => { await api.accountRemoveDevice(d.id); loadDevices(); }, `${d.name} signed out`);
        }}
      />
      <ConfirmDialog
        open={confirm === "delete"}
        title="Delete your account?"
        message={`This permanently deletes everything stored in the cloud for ${status ? whoLabel(status) : "your account"}: synced settings, dictionary, snippets, word counts and any API keys. Your other computers will be signed out. Data already on each computer stays there. This can't be undone.`}
        confirmLabel="Delete my account"
        onCancel={() => setConfirm(null)}
        onConfirm={() => {
          setConfirm(null);
          void act(async () => { await api.accountDelete(); setStatus(await api.accountStatus()); }, "Account deleted");
        }}
      />
    </>
  );
}
