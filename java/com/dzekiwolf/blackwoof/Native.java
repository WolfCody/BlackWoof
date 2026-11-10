package com.dzekiwolf.blackwoof;

/**
 * JNI entry points of libblackwoof.so.
 * Return codes: 0 = success, otherwise an ErrorKind code (see errorText in MainActivity).
 */
final class Native {
    /** Must match API_VERSION in src/android_bridge.rs. */
    static final int API_VERSION = 3;

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

    private Native() {
    }

    static native int nativeApiVersion();

    /**
     * fd must come from ParcelFileDescriptor.detachFd(). Ownership moves to native code on
     * entry, whatever the result: the caller must never close it afterwards.
     */
    static native int nativePlay(int fd);

    static native int nativePause();

    static native int nativeResume();

    static native int nativeStop();

    /** Low 8 bits: state; bits 8..15: error kind after a failure; -1 if a panic was caught. */
    static native int nativeState();

    /** 1 while a track is opening, playing or paused (the service has a reason to run), else 0. */
    static native int nativeIsActive();

    static native int nativeShutdown();
}
