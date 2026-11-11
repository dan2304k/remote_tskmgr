# rtm_viewer

RTM Viewer: the Flutter dashboard that shows live CPU/GPU/MEM/DISK stats sent by
`rtm-sender` on your PCs. Wire format: `../protocol/PROTOCOL.md`.

## Development

```sh
flutter analyze
flutter test
dart format --output=none --set-exit-if-changed lib test
```

`test/fixtures/*.json` must stay byte-identical to `../protocol/examples/*.json` (a test
checks this); copy them over when the examples change.

## Platform notes

### iOS: multicast entitlement

Receiving UDP **broadcast/multicast** on iOS 14+ requires the
`com.apple.developer.networking.multicast` entitlement, which Apple grants only on request
(<https://developer.apple.com/contact/request/networking-multicast>).
`ios/Runner/Runner.entitlements` contains it, but it is **not** wired into the Xcode
project: with an entitlement your provisioning profile lacks, device builds fail to sign.
Once Apple has approved it for your team, set *Runner > Build Settings > Code Signing
Entitlements* to `Runner/Runner.entitlements` (`CODE_SIGN_ENTITLEMENTS`) for Debug,
Release and Profile.

Without the entitlement the iOS app still works for everything that is not broadcast:
let the senders connect over **TCP** (default port 48000), or send **unicast UDP** to the
device's IP address. Allow the Local Network prompt on first launch.

### Android

The app holds a Wi-Fi multicast lock and a low-latency Wi-Fi lock while in the
foreground so broadcasts arrive promptly; both are released in the background.
