Ruston Mail for Windows (portable)
==================================

This folder runs as it is: keep it where you like and start ruston.exe. The
installer from the same release does the steps below for you and adds an
uninstaller; use one or the other, not both.

1. Windows SmartScreen may say that it protected your PC, because this build
   is not signed. Choose "More info", then "Run anyway". With Smart App
   Control turned on, Windows does not run unsigned apps at all, so this
   build cannot run there until releases are signed.

2. The first time a new message arrives, Ruston Mail adds a "Ruston Mail"
   shortcut to your Start menu: Windows shows an app's notifications only when
   one of its shortcuts carries the app's identity. Move the folder and the
   shortcut keeps pointing at the old place; delete it, and Ruston Mail adds
   it again from the new one.


Email links
-----------

To offer Ruston Mail for mailto links, run this in PowerShell from this folder,
then choose Ruston Mail for MAILTO in Settings > Apps > Default apps:

    powershell -ExecutionPolicy Bypass -File .\register-mailto.ps1 -ExecutablePath .\ruston.exe

Run unregister-mailto.ps1 the same way to remove it again. Neither script
changes your default email app.


Using ruston-cli
----------------

ruston-cli.exe is the command-line client for the same account and shares the
desktop's session. Run it from this folder, or add the folder to your PATH:

    .\ruston-cli.exe --help


Removing it
-----------

Delete this folder and the "Ruston Mail" Start menu shortcut, and run
unregister-mailto.ps1 first if you registered the email links. Your settings
and session stay; sign out first to remove the session from Windows
Credential Manager. The privacy guide lists every place Ruston Mail keeps data:
https://github.com/luiscuellar31/ruston-mail/blob/main/docs/PRIVACY.md


Checksums
---------

Every release publishes SHA256SUMS.txt. To check a download, compare its
hash with the one listed there:

    Get-FileHash .\ruston-mail-<version>-<arch>-windows-UNSIGNED.zip


Ruston Mail is free and open-source software (MIT License).
https://github.com/luiscuellar31/ruston-mail
