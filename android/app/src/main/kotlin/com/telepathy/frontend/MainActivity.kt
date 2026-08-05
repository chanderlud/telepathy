package com.telepathy.frontend

import io.flutter.embedding.android.FlutterActivity

class MainActivity: FlutterActivity() {
    init {
        System.loadLibrary("telepathy_core")
    }
}
