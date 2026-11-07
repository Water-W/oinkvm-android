package com.hachiwii.oink.android

import android.Manifest
import android.annotation.SuppressLint
import android.app.Activity
import android.content.ClipboardManager
import android.content.Intent
import android.content.pm.PackageManager
import android.graphics.Color
import android.net.Uri
import android.os.Build
import android.os.Bundle
import android.provider.Settings
import android.view.Gravity
import android.view.View
import android.view.ViewGroup
import android.webkit.JavascriptInterface
import android.webkit.WebResourceRequest
import android.webkit.WebResourceResponse
import android.webkit.WebView
import android.webkit.WebViewClient
import android.widget.Button
import android.widget.LinearLayout
import android.widget.TextView
import androidx.webkit.WebViewAssetLoader
import org.json.JSONObject

/**
 * Hosts the shared Oink interface (apps/oink, built into assets/web) in a
 * WebView. `oink-android.js` presents the Tauri IPC surface the interface
 * expects and forwards it to [Bridge].
 */
class MainActivity : Activity() {
    private lateinit var web: WebView
    private lateinit var banner: View
    private var view = "main"
    private val events: (String) -> Unit = { json ->
        runOnUiThread { web.evaluateJavascript("window.__oinkAndroid&&window.__oinkAndroid.event($json)", null) }
    }
    private val clipboardChanged = ClipboardManager.OnPrimaryClipChangedListener {
        if (hasWindowFocus()) Backend.localClipboardText(Backend.readClipboard(this))
    }

    @SuppressLint("SetJavaScriptEnabled")
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        OinkService.start(this)
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU &&
            checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS) != PackageManager.PERMISSION_GRANTED
        ) {
            requestPermissions(arrayOf(Manifest.permission.POST_NOTIFICATIONS), 1)
        }

        val assets = WebViewAssetLoader.Builder()
            .addPathHandler("/", WebAssets(this))
            .build()
        web = WebView(this).apply {
            settings.javaScriptEnabled = true
            settings.domStorageEnabled = true
            settings.allowFileAccess = false
            settings.allowContentAccess = false
            setBackgroundColor(Color.TRANSPARENT)
            webViewClient = object : WebViewClient() {
                override fun shouldInterceptRequest(view: WebView, request: WebResourceRequest): WebResourceResponse? =
                    assets.shouldInterceptRequest(request.url)

                override fun shouldOverrideUrlLoading(view: WebView, request: WebResourceRequest): Boolean {
                    if (request.url.host == HOST) return false
                    openExternal(request.url.toString())
                    return true
                }
            }
            addJavascriptInterface(Bridge(), "OinkAndroid")
        }
        banner = accessibilityBanner()
        setContentView(
            LinearLayout(this).apply {
                orientation = LinearLayout.VERTICAL
                fitsSystemWindows = true
                addView(banner, LinearLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT))
                addView(web, LinearLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, 0, 1f))
            },
        )
        Backend.addListener(events)
        getSystemService(ClipboardManager::class.java)?.addPrimaryClipChangedListener(clipboardChanged)
        load("main")
    }

    override fun onResume() {
        super.onResume()
        banner.visibility = if (InputService.instance == null) View.VISIBLE else View.GONE
    }

    override fun onWindowFocusChanged(hasFocus: Boolean) {
        super.onWindowFocusChanged(hasFocus)
        // The only moment Android lets this app read text copied elsewhere.
        if (hasFocus) Backend.localClipboardText(Backend.readClipboard(this))
    }

    override fun onDestroy() {
        Backend.removeListener(events)
        getSystemService(ClipboardManager::class.java)?.removePrimaryClipChangedListener(clipboardChanged)
        web.destroy()
        super.onDestroy()
    }

    @Deprecated("Deprecated in Java")
    override fun onBackPressed() {
        if (view != "main") {
            load("main")
        } else {
            moveTaskToBack(true)
        }
    }

    private fun load(next: String) {
        view = next
        web.loadUrl("https://$HOST/index.html?view=$next")
    }

    private fun openExternal(url: String) {
        val uri = Uri.parse(url)
        if (uri.scheme == "https" || uri.scheme == "http") {
            runCatching { startActivity(Intent(Intent.ACTION_VIEW, uri)) }
        }
    }

    private fun openAccessibilitySettings() {
        startActivity(Intent(Settings.ACTION_ACCESSIBILITY_SETTINGS).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
    }

    private fun accessibilityBanner(): View {
        val pad = (12 * resources.displayMetrics.density).toInt()
        return LinearLayout(this).apply {
            orientation = LinearLayout.HORIZONTAL
            gravity = Gravity.CENTER_VERTICAL
            setPadding(pad, pad / 2, pad, pad / 2)
            setBackgroundColor(0xFFFFF4E5.toInt())
            addView(
                TextView(context).apply {
                    setText(R.string.banner_accessibility)
                    setTextColor(0xFF5C3B00.toInt())
                },
                LinearLayout.LayoutParams(0, ViewGroup.LayoutParams.WRAP_CONTENT, 1f),
            )
            addView(
                Button(context).apply {
                    setText(R.string.banner_open)
                    setOnClickListener { openAccessibilitySettings() }
                },
            )
        }
    }

    /** Called from oink-android.js on the WebView's JavaBridge thread. */
    private inner class Bridge {
        @JavascriptInterface
        fun invoke(id: Int, command: String, args: String) {
            val params = runCatching { JSONObject(args) }.getOrDefault(JSONObject())
            if (command == "api_call") {
                val request = params.optJSONObject("request") ?: JSONObject()
                when (request.optString("method")) {
                    // The interface's "grant" for this device's control right.
                    "request_permission", "open_permission_settings" -> {
                        runOnUiThread { openAccessibilitySettings() }
                        reply(id, """{"ok":null}""")
                    }
                    else -> Backend.call(request.toString()) { reply(id, it) }
                }
                return
            }
            runOnUiThread { reply(id, shell(command, params)) }
        }

        private fun reply(id: Int, json: String) {
            runOnUiThread {
                web.evaluateJavascript("window.__oinkAndroid.reply($id, ${JSONObject.quote(json)})", null)
            }
        }
    }

    /** The Tauri shell commands the interface uses, mapped onto one Android window. */
    private fun shell(command: String, args: JSONObject): String {
        val ok = { value: Any? -> JSONObject().put("ok", value ?: JSONObject.NULL).toString() }
        return when (command) {
            "boot_info" -> ok(
                JSONObject()
                    .put("backend", if (Backend.startError == null) "agent" else "none")
                    .put("platform", "linux")
                    .put("version", BuildConfig.VERSION_NAME)
                    .put("build", JSONObject.NULL)
                    .put("open_update_failure", false),
            )
            "live_views" -> {
                Backend.setLiveViews(args.optJSONObject("views")?.toString() ?: "{}")
                ok(null)
            }
            "shell_open" -> {
                val target = args.optString("view", "main")
                if (target == "main" || target == "about") load(target)
                ok(null)
            }
            "shell_close" -> {
                if (view != "main") load("main") else moveTaskToBack(true)
                ok(null)
            }
            "shell_quit" -> {
                startService(Intent(this, OinkService::class.java).setAction("com.hachiwii.oink.android.STOP"))
                finishAndRemoveTask()
                ok(null)
            }
            "shell_open_url" -> {
                openExternal(args.optString("url"))
                ok(null)
            }
            "read_attribution" -> ok(
                runCatching { assets.open("web/" + args.optString("path").trimStart('/')).bufferedReader().readText() }
                    .getOrDefault("See https://github.com/oinkvm/oink for license and attribution notices."),
            )
            else -> ok(null)
        }
    }

    /** Serves assets/web/<path> at https://appassets.androidplatform.net/<path>. */
    private class WebAssets(private val activity: Activity) : WebViewAssetLoader.PathHandler {
        override fun handle(path: String): WebResourceResponse? {
            val name = "web/" + path.ifEmpty { "index.html" }
            return runCatching {
                WebResourceResponse(mime(name), null, activity.assets.open(name))
            }.getOrNull()
        }

        private fun mime(name: String) = when (name.substringAfterLast('.')) {
            "html" -> "text/html"
            "js" -> "text/javascript"
            "css" -> "text/css"
            "png" -> "image/png"
            "svg" -> "image/svg+xml"
            "json" -> "application/json"
            "woff2" -> "font/woff2"
            else -> "application/octet-stream"
        }
    }

    companion object {
        private const val HOST = WebViewAssetLoader.DEFAULT_DOMAIN
    }
}
