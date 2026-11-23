# What to try on your PC

Tick these off after installing. If something doesn't behave as described, note what you did and what happened; that's all I need.

## 1. Install and update
- [ ] Download Wisper-Setup.exe, run it ("More info → Run anyway" the first time). Wisper opens and a tray icon appears.
- [ ] Close the window with ✕. Wisper keeps running in the tray; clicking the tray icon brings it back.
- [ ] Restart the PC: Wisper starts on its own (Settings → General → Launch at login).
- [ ] Tray icon → Check for updates says you're up to date (later versions install themselves when you're not dictating).

## 2. Keys
- [ ] Settings → AI providers: paste the OpenAI key → Save → Test shows "It works".
- [ ] Same for the Claude key.
- [ ] Paste a deliberately wrong key → Test explains the problem in plain English. Put the right one back.

## 3. Core dictation (quit Wispr Flow first)
- [ ] In Notepad: hold **Ctrl + Win**, say a sentence, let go. A soft sound plays at start and stop; the bar shows a moving waveform, then "Cleaning up…", then your text appears in Notepad.
- [ ] The Start menu does **not** open when you release Ctrl + Win.
- [ ] Copy something first (e.g. a link), dictate, then press Ctrl+V: your original link is still on the clipboard.
- [ ] Tap Ctrl + Win twice quickly: hands-free mode with "Press Ctrl + Win to finish and paste" above the bar. Talk for a minute, tap once to finish.
- [ ] Single short tap: nothing is pasted (a hint explains hold vs double-tap).
- [ ] Press **Esc** while recording: cancelled, nothing pasted, still listed in History as cancelled.
- [ ] Dictate, and while it says "Cleaning up…" press Ctrl + Win again: you hear a short sound and see "Still finishing your last dictation". Press Esc to stop waiting, then dictate again: it works straight away.
- [ ] If you have a Hebrew keyboard layout: press Alt + Shift + Z (paste last). Your keyboard layout should NOT switch.
- [ ] Press Ctrl + Win + Right arrow (switch virtual desktop): no start sound, nothing recorded.
- [ ] Click on the desktop (no text box) and dictate: the bar says no text box was selected and offers **Copy**.
- [ ] Try in WhatsApp, Gmail (browser), Word and Claude. Each should paste correctly.

## 4. Hebrew / Yiddish
- [ ] Say: "It's a big zechus to help with the parnasa, b'ezras Hashem we'll be done before Shabbos." Check the spellings.
- [ ] Try a few of your own words. Anything wrong: add it in **Dictionary** and try again.

## 5. Formatting and modes
- [ ] "Let's meet Tuesday, no wait, Wednesday" → only Wednesday.
- [ ] Ums and ahs are removed.
- [ ] In Gmail, dictate "Hi David … thanks … Best, Asher": laid out as an email.
- [ ] Start with "as a list", pause for a moment, then say "eggs, milk, bread": it becomes a list. (The short pause matters: it tells Wisper you meant a mode switch, not a sentence that starts "As a list of…".)
- [ ] Dictate a question into Claude ("what's the capital of France"): it's typed as your question, not answered.
- [ ] Right after a dictation, click **Undo** on the bar: the original wording replaces it. Click again: back to the cleaned version.
- [ ] Dictate, type a few letters yourself, then press Alt + Shift + U: Wisper refuses to swap (so it can't undo your typing) and offers Copy instead.
- [ ] **Redo as** on the bar → pick another mode: the text is replaced.
- [ ] Style page: edit a mode's instructions, try it, then "Reset to default".
- [ ] Make a custom mode (e.g. "WhatsApp to family") and start a dictation with "as WhatsApp to family", then a short pause.
- [ ] Start a sentence normally with "In an email I sent yesterday…": nothing is cut off.

## 6. Command mode and transforms
- [ ] Select a paragraph, hold **Ctrl + Win + Alt**, say "make this shorter", let go: the selection is replaced.
- [ ] Select text and press **Win + Alt + 1** (Polish).

## 7. Long dictations and safety net
- [ ] Hands-free for 5+ minutes (read something aloud). It finishes within a few seconds of stopping; the bar shows the timer while recording.
- [ ] Turn Wi-Fi off, dictate, turn it back on: the bar says it's saved; within about a minute you get a notification that it's ready, and it's in History.
- [ ] History (home page): search for a word; open an item; play the audio; Copy; Insert again; Redo as; Delete.

## 8. Everything else
- [ ] Settings → Sounds & audio: play music, dictate. Music pauses and resumes (or mutes and unmutes, if you pick Mute).
- [ ] Snippets: add "my email address" → your email, then say "send it to my email address".
- [ ] Scratchpad: Win + Alt + S, press Dictate, speak; the text lands in the note.
- [ ] Insights shows your words, speed, time saved and streak.
- [ ] Dark theme (moon icon, top right) looks right.
- [ ] Dictionary: scroll down through the whole list (all three tabs).
- [ ] Settings → Shortcuts: click the keys (or Change), press a new combination, let go, Save. Try the new shortcut.
- [ ] Settings → Shortcuts → turn "Shortcuts enabled" off, then press Ctrl + Win: nothing happens (no sound, no bar, no recording). Turn it back on: dictation works again.
- [ ] Tray icon → "Pause shortcuts" stays in sync with the "Shortcuts enabled" setting (either one changes the other), and the choice is remembered after restarting Wisper.
- [ ] Turn on the always-visible bar: you can click and hover on things right next to it; only the small pill itself responds.
- [ ] Settings → Backup & account → Export backup, then Restore from it.

## 9. Accounts (optional)
- [ ] Settings → Account → **Sign in with Google**: the browser opens, you pick your account, the browser says you can close the tab, and Wisper shows your name.
- [ ] Choose **In my account (synced)** for your keys. Settings → AI providers says "Stored in your account and synced".
- [ ] On your second computer: install, sign in with the same Google account, choose **In my account**. Your keys, shortcuts, modes, dictionary words and snippets appear without pasting anything. Dictate: it works.
- [ ] Add a dictionary word on one computer; within a few minutes (or press **Sync now**) it appears on the other.
- [ ] Insights says "All your computers" and the word counts add up.
- [ ] On one computer switch to **This computer only**: nothing changes on the other computer.
- [ ] Your computers list shows both. Sign out the other one from here: it asks you to sign in again there, and its keys are only removed if they were the synced ones.
- [ ] Sign out: Wisper keeps working (if keys were synced, paste them again or sign back in).
- [ ] Without signing in at all, everything works exactly as before.

## 10. Speed
- [ ] Dictate a few short sentences: text appears noticeably faster than before. History shows "· live" and "Claude Haiku 5.5" for them.
- [ ] Dictate for over 30 seconds: History shows your main model (Claude Sonnet 5.5).
- [ ] Insights shows a Speed card after 3 dictations.
- [ ] Turn Wi-Fi off mid-dictation, then on, then finish: the dictation still completes (from the saved recording).
- [ ] Optional: turn on "Skip the AI for tiny dictations", say "Sounds good." — History shows "tidied on this computer".

## 11. Shortcuts anywhere and mouse shortcuts
- [ ] Click the desktop (no text box), hold Ctrl + Win, speak, let go: it records, and the bar says "No text box was selected" with Copy. The text is in History.
- [ ] Double-tap Ctrl + Win in a text box, speak, click around other windows and the desktop, then tap Ctrl + Win once: it stops every time.
- [ ] Same with the Start menu open, and with Search open (Win + S): start and stop both work, and the Start menu doesn't pop up.
- [ ] Open Task Manager (it runs as administrator), click into it and press Ctrl + Win: the bar explains that Windows hides your keys from Wisper there. While recording with Task Manager in front, the bar says so and ✓ finishes.
- [ ] Settings → Shortcuts → Dictate → Change, click the scroll wheel: it shows "Double-click" "Middle click". Save.
- [ ] Middle-click a link in the browser: it still opens in a new tab (normal clicks unaffected).
- [ ] Double-click the scroll wheel on an empty spot, speak, click it once: it stops and pastes (or goes to History if no text box). The finishing click doesn't open or scroll anything.
- [ ] Set a side button (Back) with Double-click on: single Back still goes back in the browser; double-click starts. Turn Double-click off: hold the button to talk.
- [ ] Hold Ctrl and click the scroll wheel as a shortcut: hold Ctrl + wheel to talk, let go to paste.
- [ ] Try to save just the left mouse button: it explains it needs Ctrl, Alt, Shift or Win.
- [ ] After a week of use, if a shortcut ever does nothing, send the log file (Settings → open data folder → logs): it now records hook re-installs and ignored presses (never your words).
