# SerialBridge Alpha installers

This prerelease contains four artifact groups: Windows NSIS setup (x64), macOS universal DMG (Apple Silicon and Intel), Linux DEB and AppImage (x64), and a Chromium MV3 extension ZIP.

All groups come from the same commit after Core/Node tests. Linux CI also runs real headless nodes, the web console, and the actual Chromium extension against a local HTTP fixture. Installer creation is not proof of real hardware or native UI behavior on every platform.

- Windows: run the NSIS `.exe`; WebView2 and the USB serial driver must be available.
- macOS: mount the `.dmg` and drag the app into Applications. This Alpha is not Developer ID signed or notarized; only install it if you trust this source and build.
- Linux: install the `.deb` on a supported Debian/Ubuntu system, or make the AppImage executable. The AppImage may require FUSE support on your distribution.
- Chrome/Chromium: unzip the extension into its own directory and use **Load unpacked** in developer mode. Select the directory containing `manifest.json`, then pair with the local node. This is not a Chrome Web Store package or signed CRX.

Configure exact target origins on the executing node and website permissions in the extension. The browser stays responsible for its own login state; no credentials are exported.

Physical FT232RL links, native tray/installation behavior, website-specific login compatibility and signed/notarized distribution require corresponding machines, hardware or publisher credentials. These limits do not change software test results.
