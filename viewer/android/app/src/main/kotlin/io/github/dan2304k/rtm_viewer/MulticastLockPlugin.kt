package io.github.dan2304k.rtm_viewer

import android.content.Context
import android.net.wifi.WifiManager
import android.os.Build
import android.util.Log
import io.flutter.plugin.common.BinaryMessenger
import io.flutter.plugin.common.MethodCall
import io.flutter.plugin.common.MethodChannel

/**
 * Holds a WifiManager.MulticastLock so the Wi-Fi driver passes broadcast/multicast
 * frames (UDP discovery on port 48000) up to the app. Channel: rtm_viewer/multicast_lock.
 * Also holds a low-latency WifiLock so the radio stays out of power save (otherwise
 * broadcasts only arrive at DTIM beacons, i.e. in bursts seconds apart).
 * Methods acquire (both locks; returns multicast held), release (both), isHeld
 * (multicast), isWifiLockHeld all return a Boolean and never throw.
 */
class MulticastLockPlugin(context: Context, messenger: BinaryMessenger) :
    MethodChannel.MethodCallHandler {

    private val appContext = context.applicationContext
    private val channel = MethodChannel(messenger, CHANNEL)
    private var lock: WifiManager.MulticastLock? = null
    private var wifiLock: WifiManager.WifiLock? = null

    init {
        channel.setMethodCallHandler(this)
    }

    override fun onMethodCall(call: MethodCall, result: MethodChannel.Result) {
        when (call.method) {
            "acquire" -> result.success(acquire())
            "release" -> result.success(release())
            "isHeld" -> result.success(isHeld())
            "isWifiLockHeld" -> result.success(isWifiLockHeld())
            else -> result.notImplemented()
        }
    }

    private fun wifiManager(): WifiManager? =
        appContext.getSystemService(Context.WIFI_SERVICE) as? WifiManager

    private fun acquire(): Boolean {
        acquireWifiLock()
        return acquireMulticast()
    }

    @Suppress("DEPRECATION")
    private fun acquireWifiLock(): Boolean = try {
        var l = wifiLock
        if (l == null) {
            val wifi = wifiManager()
            if (wifi != null) {
                val mode = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
                    WifiManager.WIFI_MODE_FULL_LOW_LATENCY
                } else {
                    WifiManager.WIFI_MODE_FULL_HIGH_PERF
                }
                l = wifi.createWifiLock(mode, LOCK_TAG).apply { setReferenceCounted(false) }
                wifiLock = l
            } else {
                Log.w(TAG, "WifiManager unavailable; wifi lock not acquired")
            }
        }
        if (l == null) {
            false
        } else {
            if (!l.isHeld) l.acquire()
            l.isHeld
        }
    } catch (e: Exception) {
        Log.w(TAG, "wifi lock acquire failed", e)
        false
    }

    private fun acquireMulticast(): Boolean = try {
        var l = lock
        if (l == null) {
            val wifi = wifiManager()
            if (wifi == null) {
                Log.w(TAG, "WifiManager unavailable; multicast lock not acquired")
                null
            } else {
                l = wifi.createMulticastLock(LOCK_TAG).apply { setReferenceCounted(false) }
                lock = l
                l
            }
        }
        if (l == null) {
            false
        } else {
            if (!l.isHeld) l.acquire()
            l.isHeld
        }
    } catch (e: Exception) {
        Log.w(TAG, "multicast lock acquire failed", e)
        false
    }

    private fun release(): Boolean = try {
        try {
            wifiLock?.let { if (it.isHeld) it.release() }
        } catch (e: Exception) {
            Log.w(TAG, "wifi lock release failed", e)
        }
        lock?.let { if (it.isHeld) it.release() }
        true
    } catch (e: Exception) {
        Log.w(TAG, "multicast lock release failed", e)
        false
    }

    private fun isHeld(): Boolean = try {
        lock?.isHeld ?: false
    } catch (e: Exception) {
        false
    }

    private fun isWifiLockHeld(): Boolean = try {
        wifiLock?.isHeld ?: false
    } catch (e: Exception) {
        false
    }

    /** Releases both locks and detaches the handler (engine teardown). */
    fun dispose() {
        release()
        channel.setMethodCallHandler(null)
    }

    companion object {
        const val CHANNEL = "rtm_viewer/multicast_lock"
        private const val LOCK_TAG = "rtm_viewer"
        private const val TAG = "MulticastLockPlugin"
    }
}
