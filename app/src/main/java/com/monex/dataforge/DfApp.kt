package com.monex.dataforge

import android.app.Application
import com.monex.dataforge.core.Logger

class DfApp : Application() {
    override fun onCreate() {
        super.onCreate()
        Logger.init(this)
    }
}
