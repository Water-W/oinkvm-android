package com.hachiwii.oink.android

import android.app.Activity
import android.content.Intent
import android.os.Bundle
import android.widget.Toast

/**
 * Android 10+ only lets the focused app read the clipboard. This invisible
 * activity takes focus for a moment (notification action) or receives text
 * shared from another app, hands it to the backend as a local copy, and closes.
 */
class ClipboardActivity : Activity() {
    private var handled = false

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val shared = intent?.takeIf { it.action == Intent.ACTION_SEND }?.getStringExtra(Intent.EXTRA_TEXT)
        if (shared != null) {
            deliver(shared)
        }
    }

    override fun onWindowFocusChanged(hasFocus: Boolean) {
        super.onWindowFocusChanged(hasFocus)
        if (hasFocus && !handled) {
            deliver(Backend.readClipboard(this))
        }
    }

    private fun deliver(text: String?) {
        handled = true
        if (text.isNullOrEmpty()) {
            Toast.makeText(this, R.string.clipboard_empty, Toast.LENGTH_SHORT).show()
        } else {
            if (!Backend.started) {
                OinkService.start(this)
            }
            Backend.localClipboardText(text)
            Toast.makeText(this, R.string.clipboard_sent, Toast.LENGTH_SHORT).show()
        }
        finish()
    }
}
