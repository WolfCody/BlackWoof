package com.dzekiwolf.blackwoof;

import android.app.Activity;
import android.content.Intent;
import android.graphics.Color;
import android.graphics.Typeface;
import android.net.Uri;
import android.os.Bundle;
import android.os.ParcelFileDescriptor;
import android.util.TypedValue;
import android.view.Gravity;
import android.view.View;
import android.view.ViewGroup;
import android.widget.LinearLayout;
import android.widget.TextView;

/**
 * Stage 2 test stand, not the product UI (that is Stage 5).
 * Black window, gray text, text buttons. No timers and no animations: the text is
 * refreshed only after a user action and in onResume.
 */
public final class MainActivity extends Activity {
    private static final int REQUEST_PICK = 1;
    private static final int GRAY = 0xFFB0B0B0;

    private TextView stateView;
    private String note = "";

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);

        LinearLayout outer = new LinearLayout(this);
        outer.setBackgroundColor(Color.BLACK);
        outer.setFitsSystemWindows(true);

        LinearLayout column = new LinearLayout(this);
        column.setOrientation(LinearLayout.VERTICAL);
        int pad = dp(16);
        column.setPadding(pad, pad, pad, pad);
        outer.addView(column, new LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.MATCH_PARENT));

        column.addView(label("BLACK WOOF"));
        column.addView(label("------------------------"));
        stateView = label("");
        column.addView(stateView);
        column.addView(label(""));
        column.addView(button("[ OPEN ]", new View.OnClickListener() {
            @Override
            public void onClick(View v) {
                pickFile();
            }
        }));
        column.addView(button("[ PAUSE ]", new View.OnClickListener() {
            @Override
            public void onClick(View v) {
                if (Native.LOADED) {
                    Native.nativePause();
                }
                refresh();
            }
        }));
        column.addView(button("[ RESUME ]", new View.OnClickListener() {
            @Override
            public void onClick(View v) {
                if (Native.LOADED) {
                    Native.nativeResume();
                }
                refresh();
            }
        }));
        column.addView(button("[ STOP ]", new View.OnClickListener() {
            @Override
            public void onClick(View v) {
                if (Native.LOADED) {
                    Native.nativeStop();
                }
                refresh();
            }
        }));
        column.addView(button("[ STATUS ]", new View.OnClickListener() {
            @Override
            public void onClick(View v) {
                refresh();
            }
        }));

        setContentView(outer);
        refresh();
    }

    @Override
    protected void onResume() {
        super.onResume();
        refresh();
    }

    private void pickFile() {
        Intent intent = new Intent(Intent.ACTION_OPEN_DOCUMENT);
        intent.addCategory(Intent.CATEGORY_OPENABLE);
        intent.setType("*/*");
        startActivityForResult(intent, REQUEST_PICK);
    }

    @Override
    protected void onActivityResult(int requestCode, int resultCode, Intent data) {
        super.onActivityResult(requestCode, resultCode, data);
        if (requestCode == REQUEST_PICK && resultCode == RESULT_OK
                && data != null && data.getData() != null) {
            play(data.getData());
        }
    }

    private void play(Uri uri) {
        // Check the engine before touching any descriptor.
        if (!Native.LOADED || Native.nativeApiVersion() != Native.API_VERSION) {
            note = "Audio engine is not loaded";
            refresh();
            return;
        }
        ParcelFileDescriptor pfd = null;
        try {
            pfd = getContentResolver().openFileDescriptor(uri, "r");
        } catch (Exception e) {
            pfd = null;
        }
        if (pfd == null) {
            note = "File cannot be opened";
            refresh();
            return;
        }
        // detachFd(): from here on native code owns the descriptor and closes it exactly
        // once, whatever happens. Java must not close or reuse it.
        int fd = pfd.detachFd();
        int code = Native.nativePlay(fd);
        note = code == 0 ? "" : errorText(code);
        if (code == 0) {
            startPlaybackService();
        }
        refresh();
    }

    /**
     * Starts the foreground service from this user action while the Activity is visible (the
     * Android 15/17 rule). The service ends itself when Rust reports the end of playback, and
     * leaving this screen does not stop the music.
     */
    private void startPlaybackService() {
        try {
            startForegroundService(new Intent(this, PlayerService.class));
        } catch (RuntimeException e) {
            note = "Background service could not start";
        }
    }

    private void refresh() {
        StringBuilder text = new StringBuilder();
        if (!Native.LOADED) {
            text.append("Engine: not loaded");
        } else {
            int packed = Native.nativeState();
            if (packed < 0) {
                text.append("State: ?");
            } else {
                text.append("State: ").append(stateName(packed & 0xFF));
                int error = (packed >> 8) & 0xFF;
                if (error != 0) {
                    text.append("\nError: ").append(errorText(error));
                }
            }
        }
        if (!note.isEmpty()) {
            text.append("\n").append(note);
        }
        stateView.setText(text.toString());
    }

    private static String stateName(int state) {
        switch (state) {
            case 0:
                return "IDLE";
            case 1:
                return "OPENING";
            case 2:
                return "PLAYING";
            case 3:
                return "PAUSED";
            case 4:
                return "ENDED";
            case 5:
                return "STOPPED";
            case 6:
                return "FAILED";
            default:
                return "?";
        }
    }

    /** Codes match ErrorKind in src/error.rs. */
    private static String errorText(int code) {
        switch (code) {
            case 1:
                return "Bad file handle";
            case 2:
                return "File cannot be read";
            case 3:
                return "Unknown audio format";
            case 4:
                return "No audio in file";
            case 5:
                return "Unsupported audio parameters";
            case 6:
                return "File is damaged";
            case 7:
                return "Audio output unavailable";
            case 8:
                return "Audio output refused to start";
            case 9:
                return "Audio output failed";
            case 10:
                return "Internal error";
            default:
                return "Error " + code;
        }
    }

    private TextView label(String text) {
        TextView view = new TextView(this);
        view.setText(text);
        view.setTextColor(GRAY);
        view.setTypeface(Typeface.MONOSPACE);
        view.setTextSize(TypedValue.COMPLEX_UNIT_SP, 16);
        return view;
    }

    private TextView button(String text, View.OnClickListener onClick) {
        TextView view = label(text);
        view.setMinHeight(dp(48));
        view.setGravity(Gravity.CENTER_VERTICAL);
        view.setClickable(true);
        view.setOnClickListener(onClick);
        return view;
    }

    private int dp(int value) {
        return Math.round(value * getResources().getDisplayMetrics().density);
    }
}
