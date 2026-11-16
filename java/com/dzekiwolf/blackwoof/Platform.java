package com.dzekiwolf.blackwoof;

import android.content.Context;
import android.content.pm.PackageManager;
import android.database.Cursor;
import android.net.Uri;
import android.os.Build;
import android.os.ParcelFileDescriptor;
import android.provider.MediaStore;
import android.provider.OpenableColumns;

/**
 * Static platform calls that Rust makes through JNI, because only Java can reach these Android
 * APIs. No decisions here, and none of these calls may call back into Rust synchronously.
 */
final class Platform {
    private static volatile Context app;

    private Platform() {
    }

    static void init(Context context) {
        app = context.getApplicationContext();
    }

    static String audioPermission() {
        return Build.VERSION.SDK_INT >= 33
                ? "android.permission.READ_MEDIA_AUDIO"
                : "android.permission.READ_EXTERNAL_STORAGE";
    }

    static boolean hasAudioPermission() {
        Context context = app;
        return context != null
                && context.checkSelfPermission(audioPermission()) == PackageManager.PERMISSION_GRANTED;
    }

    /**
     * Opens the URI and returns a detached descriptor, or -1. After detachFd() the descriptor
     * belongs to Rust; on any failure before that it is closed here.
     */
    static int openAudio(String uri) {
        Context context = app;
        if (context == null) {
            return -1;
        }
        ParcelFileDescriptor pfd = null;
        try {
            pfd = context.getContentResolver().openFileDescriptor(Uri.parse(uri), "r");
            if (pfd == null) {
                return -1;
            }
            return pfd.detachFd();
        } catch (Exception e) {
            if (pfd != null) {
                try {
                    pfd.close();
                } catch (Exception ignored) {
                    // nothing more to do
                }
            }
            return -1;
        }
    }

    /**
     * One query, made only when the user opens the list or presses refresh: id, title and
     * artist, one track per line, fields separated by a tab. Null if the query failed.
     */
    static String queryTracks() {
        Context context = app;
        if (context == null) {
            return null;
        }
        Cursor cursor = null;
        try {
            cursor = context.getContentResolver().query(
                    MediaStore.Audio.Media.EXTERNAL_CONTENT_URI,
                    new String[] {
                            MediaStore.Audio.Media._ID,
                            MediaStore.Audio.Media.TITLE,
                            MediaStore.Audio.Media.ARTIST },
                    MediaStore.Audio.Media.IS_MUSIC + " != 0",
                    null,
                    MediaStore.Audio.Media.TITLE + " COLLATE NOCASE ASC");
            if (cursor == null) {
                return null;
            }
            StringBuilder out = new StringBuilder();
            while (cursor.moveToNext()) {
                out.append(cursor.getLong(0)).append('\t')
                        .append(clean(cursor.getString(1))).append('\t')
                        .append(clean(cursor.getString(2))).append('\n');
            }
            return out.toString();
        } catch (Exception e) {
            return null;
        } finally {
            if (cursor != null) {
                cursor.close();
            }
        }
    }

    /** File name of a URI handed over by Android, or null. */
    static String displayName(String uri) {
        Context context = app;
        if (context == null) {
            return null;
        }
        Cursor cursor = null;
        try {
            cursor = context.getContentResolver().query(
                    Uri.parse(uri), new String[] { OpenableColumns.DISPLAY_NAME }, null, null, null);
            if (cursor != null && cursor.moveToFirst()) {
                return cursor.getString(0);
            }
        } catch (Exception e) {
            // the name simply stays unknown
        } finally {
            if (cursor != null) {
                cursor.close();
            }
        }
        return null;
    }

    private static String clean(String s) {
        return s == null ? "" : s.replace('\t', ' ').replace('\n', ' ').replace('\r', ' ');
    }
}
