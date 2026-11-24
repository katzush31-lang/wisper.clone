# Wisper

Voice dictation for Windows. Hold a key, speak, and polished text appears wherever your cursor is: email, WhatsApp, Word, Claude, anywhere.

Built as a personal replacement for Wispr Flow, with no time limit, Hebrew/Yiddish words spelled properly (Ashkenazi spelling), and every dictation saved before anything is sent online.

## Install

1. Download **[Wisper-Setup.exe](https://github.com/katzush31-lang/wisper.clone/releases/latest/download/Wisper-Setup.exe)**.
2. Double-click it. Windows may say *"Windows protected your PC"* because the app isn't signed with a paid certificate. Click **More info → Run anyway**. You only need to do this once; updates install themselves.
3. Wisper opens and adds a small icon to the system tray (bottom-right, near the clock).

## First-time setup (2 minutes)

Wisper uses your own AI accounts, so you pay the providers directly (typically a few pounds a month).

1. **OpenAI key** (turns your voice into text): sign in at [platform.openai.com/api-keys](https://platform.openai.com/api-keys), create a key, and add a few dollars of credit under Billing.
2. **Claude key** (tidies up the text): sign in at [platform.claude.com/settings/keys](https://platform.claude.com/settings/keys), create a key, and add credit under Billing.
3. In Wisper, open **Settings → AI providers**, paste each key, press **Save**, then **Test**. Keys are stored in Windows Credential Manager, never in a file.

If Wispr Flow is still running, quit it first. Both apps use **Ctrl + Win** by default. You can change Wisper's shortcuts in **Settings → Shortcuts**.

## How to use it

| What you want | Do this |
|---|---|
| Dictate a sentence | Hold **Ctrl + Win**, speak, let go |
| Dictate hands-free (long dictations) | Tap **Ctrl + Win** twice quickly, speak as long as you like, tap once more to finish. Or use **Ctrl + Win + Space** |
| Cancel | **Esc**, or the ✕ on the bar. The recording is still kept in History |
| Edit text by voice | Select text, hold **Ctrl + Win + Alt**, say what to do ("make this shorter", "turn this into bullet points"), let go |
| Use a format just once | Start with "as an email", "as a list" or "as" + one of your modes, then a short pause ("As an email, hi David…") |
| Get the original wording back | Click **Undo** on the bar right after, or press **Alt + Shift + U** (press again to swap back) |
| Redo in another style | Click **Redo as** on the bar, or press **Alt + Shift + R** |
| Paste your last dictation again | **Alt + Shift + Z** (copy it: **Alt + Shift + X**) |
| Rewrite selected text without speaking | **Win + Alt + 1** (Polish), **Win + Alt + 2** (Prompt Engineer). See the Transforms page |
| Open the Scratchpad | **Win + Alt + S** |

Wisper starts recording the instant you press the keys, so you don't need to pause before speaking. There is no time limit. Long dictations are transcribed in pieces while you're still talking, so they finish quickly.

## Speed settings

In **Settings → AI providers**:
- **Live transcription** (on): Wisper transcribes while you talk, so text is ready as soon as you stop. Your recording is still saved; if the live connection drops, Wisper finishes from the recording.
- **Use a faster model for short dictations** (on, under 30 seconds): short dictations are tidied by Claude Haiku, about twice as fast, with the same results in all our quality tests. Long ones use your main model.
- **Tidy up while you talk** (on): long dictations are cleaned up in pieces as you speak, so the text is ready almost as soon as you stop. If you correct yourself near the end ("no wait…"), use a list or email layout, or anything else that needs the whole text, Wisper cleans it all at once as before.
- **Skip the AI for tiny dictations** (off): very short sentences with nothing to fix are tidied instantly on your computer.

**Insights** shows how long you wait after you stop talking.

## Using Wisper on more than one computer (optional)

You don't need an account: Wisper works fully on its own. If you use it on several computers, open **Settings → Account → Sign in with Google**. Your settings, shortcuts, modes, dictionary, snippets and word counts then sync between your computers. History, recordings and Scratchpad notes stay on each computer.

The first time you sign in on a computer, choose where that computer keeps its API keys:
- **In my account (synced):** keys are encrypted on Wisper's server and shared with your other computers that also choose this, so you paste them only once.
- **This computer only:** keys never leave this PC.

**Remove keys from your account** deletes the account's copy; every computer keeps the keys it already has. Signing out never deletes keys that exist only on that computer.

## If something goes wrong

- **Nothing pastes:** click into a text box first. If no text box is selected, the bar shows your text with a **Copy** button, and it's also in History. Apps running as administrator can't receive pasted text from normal apps; that's a Windows rule.
- **"No internet" or "provider busy":** your dictation is saved. Wisper retries automatically and processes it when you're back online. You'll get a notification when it's ready.
- **"Your API key isn't working":** open Settings → AI providers and press **Test** to see what's wrong (usually an expired key or no credit). Dictations that failed because of a key are retried after you fix it.
- **Words keep coming out wrong:** add them in **Dictionary**. For names that come out several ways, Wisper suggests a spelling for you to approve.
- **First words cut off with Bluetooth headphones:** Bluetooth headsets take a moment to switch on. Use the laptop microphone, or turn on **Settings → General → Keep microphone ready**.
- Everything Wisper stores (history, audio, settings, logs) is in the folder opened by **Settings → Data & privacy → Open data folder**.

## For developers

See [CLAUDE.md](CLAUDE.md) for the layout and commands, [docs/PLAN.md](docs/PLAN.md) for decisions, and [PROGRESS.md](PROGRESS.md) for status. Every push to `main` builds the Windows installer on GitHub Actions and publishes a release that the app auto-updates from.
