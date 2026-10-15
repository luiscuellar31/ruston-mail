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


Using ruston-cli
----------------

The command-line client ships inside the app. To run it as "ruston-cli", link it
into a folder on your PATH, for example:

    sudo ln -sf "/Applications/Ruston Mail.app/Contents/MacOS/ruston-cli" /usr/local/bin/ruston-cli

macOS checks ruston-cli the first time it runs, as it does the app. The xattr
command above approves both.


About Keychain and Saved Passwords
----------------------------------

Ruston Mail stores its session tokens and key passphrase in the macOS Keychain.
An ad-hoc signature identifies one build only, so macOS treats every new version
as a different app and asks again before it can read the saved session. When
prompted, select "Always Allow" to answer once for that version.


Checksums
---------

Every release publishes SHA256SUMS.txt beside the disk images. To check a
download, run this in the folder that holds both files:

    shasum -a 256 -c SHA256SUMS.txt --ignore-missing


Ruston Mail is free and open-source software (MIT License).
https://github.com/luiscuellar31/ruston-mail
