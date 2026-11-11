package io.github.dan2304k.rtm_viewer

import io.flutter.embedding.android.FlutterActivity
import io.flutter.embedding.engine.FlutterEngine

class MainActivity : FlutterActivity() {
    private var multicastLock: MulticastLockPlugin? = null

    override fun configureFlutterEngine(flutterEngine: FlutterEngine) {
        super.configureFlutterEngine(flutterEngine)
        multicastLock?.dispose()
        multicastLock = MulticastLockPlugin(this, flutterEngine.dartExecutor.binaryMessenger)
    }

    override fun cleanUpFlutterEngine(flutterEngine: FlutterEngine) {
        multicastLock?.dispose()
        multicastLock = null
        super.cleanUpFlutterEngine(flutterEngine)
    }
}
