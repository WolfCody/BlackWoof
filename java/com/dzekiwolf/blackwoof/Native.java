package com.dzekiwolf.blackwoof;

/** JNI entry points of libblackwoof.so. All decisions live in Rust; these only carry input. */
final class Native {
    /** Must match API_VERSION in src/android_bridge.rs. */
    static final int API_VERSION = 4;

    static final boolean LOADED;

    static {
        boolean ok;
        try {
            System.loadLibrary("blackwoof");
            ok = true;
        } catch (UnsatisfiedLinkError e) {
            ok = false;
        }
        LOADED = ok;
    }

    /** Flags returned by nativeClick / nativeCommand / nativeOpenExternal. */
    static final int EFFECT_START_SERVICE = 1;
    static final int EFFECT_REQUEST_PERMISSION = 2;

    /** Commands for nativeCommand (src/playback_controller.rs). */
    static final int CMD_PLAY = 1;
    static final int CMD_PAUSE = 2;
    static final int CMD_TOGGLE = 3;
    static final int CMD_NEXT = 4;
    static final int CMD_PREVIOUS = 5;
    static final int CMD_STOP = 6;

    /** Button ids that Java itself must know (src/ui.rs); all others come from the screen text. */
    static final int ID_BACK = 10;
    static final int ID_ITEM_BASE = 1000;

    /** nativeFocus value for "the audio focus request was refused". */
    static final int FOCUS_DENIED = 100;

    private Native() {
    }

    static native int nativeApiVersion();

    /** Once per process, with the private files directory (where the last playback is saved). */
    static native int nativeInit(String filesDir);

    /** A button or list row was pressed; returns EFFECT_* flags. */
    static native int nativeClick(int id);

    /** The whole screen as lines, see src/ui.rs. */
    static native String nativeScreen();

    static native String nativeItem(int index);

    static native int nativeItemCount();

    static native int nativeCommand(int command);

    static native int nativeFocus(int change);

    static native int nativePermissionResult(int granted);

    /** Android opened an audio URI with BLACK WOOF; returns EFFECT_* flags. */
    static native int nativeOpenExternal(String uri);

    /** The service started; Rust pushes the current picture. 0 means: nothing to keep alive. */
    static native int nativeServiceReady();

    /** Clean close: Rust writes down where playback is. */
    static native int nativeSave();

    static native int nativeShutdown();
}
