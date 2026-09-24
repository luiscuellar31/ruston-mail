Opening Ruston Mail for the first time
======================================

1. Drag "Ruston Mail" onto the "Applications" folder in this window.

2. Open your Applications folder and launch Ruston Mail. Because this build is
   signed with an ad-hoc signature (no Apple Developer Program enrollment),
   macOS Gatekeeper may report that it cannot check the app for malicious software.

3. To approve it in macOS:
   - Open System Settings -> Privacy & Security.
   - Scroll down to the Security section.
   - Click "Open Anyway" next to the message about Ruston Mail, and confirm.
   (On macOS 12 and 13: System Preferences -> Security & Privacy -> General).

4. Alternatively, you can approve it directly from the Terminal:

       xattr -dr com.apple.quarantine "/Applications/Ruston Mail.app"


About Keychain and Saved Passwords
----------------------------------

Ruston Mail stores session tokens and mailbox passwords in the native macOS Keychain.
With the ad-hoc code signature applied to this application bundle, macOS associates
your keychain authorization with Ruston Mail's bundle identifier (com.luiscuellar.ruston-mail).
When prompted by macOS to allow access to the keychain, select "Always Allow".


Checksums
---------

Every build publishes a SHA-256 checksum so you can verify the integrity of the disk image:

    shasum -a 256 ~/Downloads/ruston-mail-*.dmg


Ruston Mail is free and open-source software (MIT License).
https://github.com/luiscuellar31/ruston-mail
