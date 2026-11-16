package com.dzekiwolf.blackwoof;

import android.app.Activity;
import android.content.Intent;
import android.content.pm.PackageManager;
import android.graphics.Color;
import android.graphics.Typeface;
import android.net.Uri;
import android.os.Bundle;
import android.util.TypedValue;
import android.view.Gravity;
import android.view.View;
import android.view.ViewGroup;
import android.widget.AdapterView;
import android.widget.BaseAdapter;
import android.widget.LinearLayout;
import android.widget.ListView;
import android.widget.TextView;

/**
 * A thin renderer. Rust composes the whole text screen (src/ui.rs) and decides what every
 * press means; this class draws the lines it is given, forwards presses, and runs the few
 * things only an Activity can do (permission dialog, starting the service).
 *
 * Black window, plain text, no animation, no timer: the screen is redrawn only when Rust says
 * something changed (onUiChanged) and when the Activity comes back to the front.
 */
public final class MainActivity extends Activity {
    private static final int REQUEST_AUDIO = 1;
    private static final int GRAY = 0xFFB0B0B0;
    private static final int RED = 0xFFFF3B30;

    private static volatile MainActivity current;

    private LinearLayout host;
    private ListView list;
    private boolean onLibrary;

    /** Called from Rust (through JNI) when what the screen shows has changed. */
    static void onUiChanged() {
        final MainActivity activity = current;
        if (activity != null) {
            activity.runOnUiThread(new Runnable() {
                @Override
                public void run() {
                    activity.render();
                }
            });
        }
    }

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);
        Platform.init(this);
        if (Native.LOADED) {
            Native.nativeInit(getFilesDir().getAbsolutePath());
        }

        LinearLayout outer = new LinearLayout(this);
        outer.setBackgroundColor(Color.BLACK);
        outer.setFitsSystemWindows(true);
        host = new LinearLayout(this);
        host.setOrientation(LinearLayout.VERTICAL);
        int pad = dp(16);
        host.setPadding(pad, pad, pad, pad);
        outer.addView(host, new LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.MATCH_PARENT));
        setContentView(outer);

        if (savedInstanceState == null) {
            handleIntent(getIntent());
        }
        render();
    }

    @Override
    protected void onNewIntent(Intent intent) {
        super.onNewIntent(intent);
        setIntent(intent);
        handleIntent(intent);
    }

    @Override
    protected void onResume() {
        super.onResume();
        current = this;
        render();
    }

    @Override
    protected void onPause() {
        if (current == this) {
            current = null;
        }
        super.onPause();
    }

    @Override
    public void onBackPressed() {
        if (onLibrary && Native.LOADED) {
            apply(Native.nativeClick(Native.ID_BACK));
        } else {
            super.onBackPressed();
        }
    }

    @Override
    public void onRequestPermissionsResult(int requestCode, String[] permissions, int[] grantResults) {
        super.onRequestPermissionsResult(requestCode, permissions, grantResults);
        if (requestCode == REQUEST_AUDIO && Native.LOADED) {
            boolean granted = grantResults.length > 0
                    && grantResults[0] == PackageManager.PERMISSION_GRANTED;
            Native.nativePermissionResult(granted ? 1 : 0);
        }
    }

    /** Android opened an audio file with BLACK WOOF: hand the URI to Rust, never to another app. */
    private void handleIntent(Intent intent) {
        if (intent == null || !Native.LOADED || !Intent.ACTION_VIEW.equals(intent.getAction())) {
            return;
        }
        Uri uri = intent.getData();
        if (uri == null || !"content".equals(uri.getScheme())) {
            return;
        }
        try {
            getContentResolver().takePersistableUriPermission(uri, Intent.FLAG_GRANT_READ_URI_PERMISSION);
        } catch (Exception e) {
            // Not every provider offers a lasting grant; the file may then not reopen later.
        }
        apply(Native.nativeOpenExternal(uri.toString()));
    }

    private void click(int id) {
        if (Native.LOADED) {
            apply(Native.nativeClick(id));
        }
    }

    /** Things only an Activity can do, as Rust asked. */
    private void apply(int effects) {
        if ((effects & Native.EFFECT_START_SERVICE) != 0) {
            startPlaybackService();
        }
        if ((effects & Native.EFFECT_REQUEST_PERMISSION) != 0) {
            requestPermissions(new String[] { Platform.audioPermission() }, REQUEST_AUDIO);
        }
    }

    /**
     * Starts the foreground service from a user action while this Activity is visible (the
     * Android 15/17 rule). The service ends itself when Rust says playback is over, and leaving
     * this screen does not stop the music.
     */
    private void startPlaybackService() {
        try {
            startForegroundService(new Intent(this, PlayerService.class));
        } catch (RuntimeException e) {
            // Playback continues without the service; Android may limit it in the background.
        }
    }

    private void render() {
        String screen = Native.LOADED ? Native.nativeScreen() : null;
        if (screen == null) {
            screen = "S|gray\nT|BLACK WOOF\nT|Audio engine is not loaded";
        }
        int first = 0;
        int top = 0;
        if (list != null) {
            first = list.getFirstVisiblePosition();
            View row = list.getChildAt(0);
            top = row == null ? 0 : row.getTop();
        }
        host.removeAllViews();
        list = null;

        String[] lines = screen.split("\n");
        int color = GRAY;
        onLibrary = false;
        for (String line : lines) {
            if (line.startsWith("S|")) {
                color = line.endsWith("red") ? RED : GRAY;
            } else if (line.startsWith("K|")) {
                onLibrary = line.endsWith("library");
            }
        }
        for (String line : lines) {
            if (line.startsWith("T|")) {
                host.addView(label(line.substring(2), color));
            } else if (line.startsWith("B|")) {
                String[] parts = line.split("\\|", 3);
                if (parts.length == 3) {
                    host.addView(button(parts[2], id(parts[1]), color));
                }
            } else if (line.startsWith("H|")) {
                String[] parts = line.split("\\|");
                LinearLayout row = new LinearLayout(this);
                row.setOrientation(LinearLayout.HORIZONTAL);
                for (int i = 1; i + 1 < parts.length; i += 2) {
                    row.addView(button(parts[i + 1], id(parts[i]), color),
                            new LinearLayout.LayoutParams(
                                    0, ViewGroup.LayoutParams.WRAP_CONTENT, 1f));
                }
                host.addView(row);
            } else if (line.startsWith("LIST|")) {
                list = new ListView(this);
                list.setDivider(null);
                list.setAdapter(new Rows(color));
                list.setOnItemClickListener(new AdapterView.OnItemClickListener() {
                    @Override
                    public void onItemClick(AdapterView<?> parent, View view, int position, long rowId) {
                        click(Native.ID_ITEM_BASE + position);
                    }
                });
                host.addView(list, new LinearLayout.LayoutParams(
                        ViewGroup.LayoutParams.MATCH_PARENT, 0, 1f));
                list.setSelectionFromTop(first, top);
            }
        }
    }

    private static int id(String text) {
        try {
            return Integer.parseInt(text.trim());
        } catch (NumberFormatException e) {
            return -1;
        }
    }

    private TextView label(String text, int color) {
        TextView view = new TextView(this);
        view.setText(text);
        view.setTextColor(color);
        view.setTypeface(Typeface.MONOSPACE);
        view.setTextSize(TypedValue.COMPLEX_UNIT_SP, 16);
        return view;
    }

    private TextView button(String text, final int id, int color) {
        TextView view = label(text, color);
        view.setMinHeight(dp(48));
        view.setGravity(Gravity.CENTER_VERTICAL);
        view.setClickable(true);
        view.setOnClickListener(new View.OnClickListener() {
            @Override
            public void onClick(View v) {
                click(id);
            }
        });
        return view;
    }

    private int dp(int value) {
        return Math.round(value * getResources().getDisplayMetrics().density);
    }

    /** Rows of the music list: the text of row i is asked from Rust when the row is drawn. */
    private final class Rows extends BaseAdapter {
        private final int color;

        Rows(int color) {
            this.color = color;
        }

        @Override
        public int getCount() {
            return Native.LOADED ? Native.nativeItemCount() : 0;
        }

        @Override
        public Object getItem(int position) {
            return null;
        }

        @Override
        public long getItemId(int position) {
            return position;
        }

        @Override
        public View getView(int position, View convertView, ViewGroup parent) {
            TextView view = convertView instanceof TextView
                    ? (TextView) convertView : label("", color);
            view.setMinHeight(dp(40));
            view.setGravity(Gravity.CENTER_VERTICAL);
            String text = Native.LOADED ? Native.nativeItem(position) : "";
            view.setText(text == null ? "" : text);
            view.setTextColor(color);
            return view;
        }
    }
}
