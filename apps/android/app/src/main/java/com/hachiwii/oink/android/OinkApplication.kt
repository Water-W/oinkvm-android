package com.hachiwii.oink.android

import android.app.Application

class OinkApplication : Application() {
    override fun onCreate() {
        super.onCreate()
        Backend.init(this)
    }
}
