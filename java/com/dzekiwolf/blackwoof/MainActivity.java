package com.dzekiwolf.blackwoof;

import android.Manifest;
import android.app.Activity;
import android.content.ContentUris;
import android.content.Intent;
import android.content.pm.PackageManager;
import android.database.Cursor;
import android.graphics.Color;
import android.graphics.Typeface;
import android.net.Uri;
import android.os.Build;
import android.os.Bundle;
import android.os.Handler;
import android.os.Looper;
import android.os.ParcelFileDescriptor;
import android.provider.MediaStore;
import android.util.TypedValue;
import android.view.Gravity;
import android.view.View;
import android.view.ViewGroup;
import android.widget.ArrayAdapter;
import android.widget.LinearLayout;
import android.widget.ListView;
import android.widget.TextView;

import java.util.ArrayList;

/**
 * BLACK WOOF minimal text UI and Stage 3.1 local Tracks library.
 * MediaStore is queried only after the user opens/rescans the library.
 */
public final class MainActivity extends Activity {
    private static final int REQUEST_PICK = 1;
    private static final int REQUEST_AUDIO_PERMISSION = 2;
    private static final int GRAY = 0xFFB0B0B0;
    private static final int WHITE = 0xFFE0E0E0;

    private final Handler mainHandler = new Handler(Looper.getMainLooper());
    private final ArrayList<Track> tracks = new ArrayList<>();
    private LinearLayout root;
    private TextView stateView;
    private TextView libraryStatus;
    private ListView trackList;
    private String note = "";
    private String currentTitle = "";
    private String currentArtist = "";
    private boolean showingLibrary;
    private boolean scanInProgress;

    private static final class Track {
        final long id;
        final String title;
        final String artist;
        final String album;
        final long duration;

        Track(long id, String title, String artist, String album, long duration) {
            this.id = id;
            this.title = title;
            this.artist = artist;
            this.album = album;
            this.duration = duration;
        }

        Uri uri() {
            return ContentUris.withAppendedId(
                    MediaStore.Audio.Media.EXTERNAL_CONTENT_URI, id);
        }

        String displayText() {
            String by = artist == null || artist.isEmpty() ? "Unknown artist" : artist;
            return title + "\n" + by;
        }
    }

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);
        showPlayer();
    }

    @Override
    protected void onResume() {
        super.onResume();
        if (!showingLibrary) refreshPlayer();
    }

    @Override
    protected void onDestroy() {
        super.onDestroy();
        if (isFinishing() && Native.LOADED) Native.nativeShutdown();
    }

    private void showPlayer() {
        showingLibrary = false;
        root = new LinearLayout(this);
        root.setOrientation(LinearLayout.VERTICAL);
        root.setBackgroundColor(Color.BLACK);
        root.setPadding(dp(16), dp(16), dp(16), dp(16));

        root.addView(label("BLACK WOOF"));
        root.addView(label("------------------------"));
        stateView = label("");
        root.addView(stateView);
        root.addView(label(""));
        root.addView(button("[ LIBRARY ]", new View.OnClickListener() {
            @Override public void onClick(View v) { openLibrary(); }
        }));
        root.addView(button("[ OPEN ]", new View.OnClickListener() {
            @Override public void onClick(View v) { pickFile(); }
        }));
        root.addView(button("[ PAUSE ]", new View.OnClickListener() {
            @Override public void onClick(View v) {
                if (Native.LOADED) Native.nativePause();
                refreshPlayer();
            }
        }));
        root.addView(button("[ RESUME ]", new View.OnClickListener() {
            @Override public void onClick(View v) {
                if (Native.LOADED) Native.nativeResume();
                refreshPlayer();
            }
        }));
        root.addView(button("[ STOP ]", new View.OnClickListener() {
            @Override public void onClick(View v) {
                if (Native.LOADED) Native.nativeStop();
                refreshPlayer();
            }
        }));
        root.addView(button("[ STATUS ]", new View.OnClickListener() {
            @Override public void onClick(View v) { refreshPlayer(); }
        }));
        setContentView(root);
        refreshPlayer();
    }

    private void showLibrary(String status) {
        showingLibrary = true;
        root = new LinearLayout(this);
        root.setOrientation(LinearLayout.VERTICAL);
        root.setBackgroundColor(Color.BLACK);
        root.setPadding(dp(16), dp(16), dp(16), dp(8));
        root.addView(label("BLACK WOOF / LIBRARY"));
        root.addView(label("------------------------"));
        root.addView(button("[ BACK ]", new View.OnClickListener() {
            @Override public void onClick(View v) { showPlayer(); }
        }));
        root.addView(button("[ RESCAN ]", new View.OnClickListener() {
            @Override public void onClick(View v) { requestLibraryAccessAndScan(); }
        }));
        libraryStatus = label(status);
        root.addView(libraryStatus);
        trackList = new ListView(this);
        trackList.setBackgroundColor(Color.BLACK);
        trackList.setDivider(null);
        trackList.setCacheColorHint(Color.BLACK);
        trackList.setOnItemClickListener((parent, view, position, id) -> {
            if (position >= 0 && position < tracks.size()) {
                Track selected = tracks.get(position);
                play(selected.uri(), selected.title, selected.artist);
                showPlayer();
            }
        });
        root.addView(trackList, new LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT, 0, 1.0f));
        setContentView(root);
        if (tracks.isEmpty() && status == null) requestLibraryAccessAndScan();
        else bindTracks();
    }

    private void openLibrary() {
        showLibrary("Requesting music access...");
        requestLibraryAccessAndScan();
    }

    private String audioPermission() {
        return Build.VERSION.SDK_INT >= 33
                ? Manifest.permission.READ_MEDIA_AUDIO
                : Manifest.permission.READ_EXTERNAL_STORAGE;
    }

    private boolean hasAudioPermission() {
        return Build.VERSION.SDK_INT < 23
                || checkSelfPermission(audioPermission()) == PackageManager.PERMISSION_GRANTED;
    }

    private void requestLibraryAccessAndScan() {
        if (!hasAudioPermission()) {
            requestPermissions(new String[]{audioPermission()}, REQUEST_AUDIO_PERMISSION);
            return;
        }
        scanLibrary();
    }

    @Override
    public void onRequestPermissionsResult(int requestCode, String[] permissions, int[] results) {
        super.onRequestPermissionsResult(requestCode, permissions, results);
        if (requestCode == REQUEST_AUDIO_PERMISSION) {
            if (hasAudioPermission()) scanLibrary();
            else if (libraryStatus != null) {
                libraryStatus.setText("Music access denied. Use OPEN to choose a file.");
            }
        }
    }

    /** One bounded MediaStore query on a temporary worker, started only by user action. */
    private void scanLibrary() {
        if (!showingLibrary || scanInProgress) return;
        scanInProgress = true;
        if (libraryStatus != null) libraryStatus.setText("Scanning...");
        Thread worker = new Thread(new Runnable() {
            @Override public void run() {
                ArrayList<Track> found = new ArrayList<>();
                String error = null;
                String[] projection = {
                        MediaStore.Audio.Media._ID,
                        MediaStore.Audio.Media.TITLE,
                        MediaStore.Audio.Media.ARTIST,
                        MediaStore.Audio.Media.ALBUM,
                        MediaStore.Audio.Media.DURATION
                };
                String selection = MediaStore.Audio.Media.IS_MUSIC + " != 0";
                String sort = MediaStore.Audio.Media.TITLE + " COLLATE NOCASE ASC";
                Uri collection = Build.VERSION.SDK_INT >= 29
                        ? MediaStore.Audio.Media.getContentUri(MediaStore.VOLUME_EXTERNAL)
                        : MediaStore.Audio.Media.EXTERNAL_CONTENT_URI;
                try (Cursor cursor = getContentResolver().query(
                        collection, projection, selection, null, sort)) {
                    if (cursor != null) {
                        int idCol = cursor.getColumnIndexOrThrow(MediaStore.Audio.Media._ID);
                        int titleCol = cursor.getColumnIndexOrThrow(MediaStore.Audio.Media.TITLE);
                        int artistCol = cursor.getColumnIndexOrThrow(MediaStore.Audio.Media.ARTIST);
                        int albumCol = cursor.getColumnIndexOrThrow(MediaStore.Audio.Media.ALBUM);
                        int durationCol = cursor.getColumnIndexOrThrow(MediaStore.Audio.Media.DURATION);
                        while (cursor.moveToNext()) {
                            long id = cursor.getLong(idCol);
                            String title = cursor.getString(titleCol);
                            if (title == null || title.trim().isEmpty()) title = "Unknown title";
                            found.add(new Track(id, title, cursor.getString(artistCol),
                                    cursor.getString(albumCol), cursor.getLong(durationCol)));
                        }
                    }
                } catch (SecurityException e) {
                    error = "Music access denied";
                } catch (Exception e) {
                    error = "Library scan failed";
                }
                final ArrayList<Track> result = found;
                final String failure = error;
                mainHandler.post(new Runnable() {
                    @Override public void run() {
                        scanInProgress = false;
                        if (!showingLibrary || isFinishing()) return;
                        tracks.clear();
                        tracks.addAll(result);
                        if (failure != null) libraryStatus.setText(failure);
                        else if (tracks.isEmpty()) libraryStatus.setText("No tracks found");
                        else libraryStatus.setText("Tracks: " + tracks.size());
                        bindTracks();
                    }
                });
            }
        }, "blackwoof-library-query");
        worker.start();
    }

    private void bindTracks() {
        if (trackList == null) return;
        ArrayAdapter<Track> adapter = new ArrayAdapter<Track>(
                this, android.R.layout.simple_list_item_1, tracks) {
            @Override public View getView(int position, View convertView, ViewGroup parent) {
                TextView row = (TextView) super.getView(position, convertView, parent);
                row.setText(tracks.get(position).displayText());
                row.setTextColor(WHITE);
                row.setTypeface(Typeface.MONOSPACE);
                row.setTextSize(TypedValue.COMPLEX_UNIT_SP, 15);
                row.setPadding(dp(4), dp(10), dp(4), dp(10));
                row.setMinHeight(dp(52));
                row.setBackgroundColor(Color.BLACK);
                return row;
            }
        };
        trackList.setAdapter(adapter);
    }

    private void pickFile() {
        Intent intent = new Intent(Intent.ACTION_OPEN_DOCUMENT);
        intent.addCategory(Intent.CATEGORY_OPENABLE);
        intent.setType("audio/*");
        startActivityForResult(intent, REQUEST_PICK);
    }

    protected void onActivityResult(int requestCode, int resultCode, Intent data) {
        super.onActivityResult(requestCode, resultCode, data);
        if (requestCode == REQUEST_PICK && resultCode == RESULT_OK
                && data != null && data.getData() != null) play(data.getData(), "", "");
    }

    private void play(Uri uri, String title, String artist) {
        if (!Native.LOADED || Native.nativeApiVersion() != Native.API_VERSION) {
            note = "Audio engine is not loaded";
            refreshPlayer();
            return;
        }
        ParcelFileDescriptor pfd = null;
        try {
            pfd = getContentResolver().openFileDescriptor(uri, "r");
        } catch (Exception ignored) {
            // The URI may have been removed or its provider may be unavailable.
        }
        if (pfd == null) {
            note = "File cannot be opened";
            refreshPlayer();
            return;
        }
        // Ownership transfers to Rust at nativePlay entry; Java must not close this FD.
        int fd;
        try {
            fd = pfd.detachFd();
        } catch (RuntimeException e) {
            try { pfd.close(); } catch (Exception ignored) { }
            note = "File descriptor unavailable";
            refreshPlayer();
            return;
        }
        int code = Native.nativePlay(fd);
        note = code == 0 ? "" : errorText(code);
        if (code == 0) {
            currentTitle = title == null ? "" : title;
            currentArtist = artist == null ? "" : artist;
            startPlaybackService();
        }
        refreshPlayer();
    }

    private void startPlaybackService() {
        try {
            startForegroundService(new Intent(this, PlayerService.class));
        } catch (RuntimeException e) {
            note = "Background service could not start";
        }
    }

    private void refreshPlayer() {
        if (stateView == null || showingLibrary) return;
        StringBuilder text = new StringBuilder();
        if (!Native.LOADED) text.append("Engine: not loaded");
        else {
            int packed = Native.nativeState();
            if (packed < 0) text.append("State: ?");
            else {
                text.append("State: ").append(stateName(packed & 0xFF));
                int error = (packed >> 8) & 0xFF;
                if (error != 0) text.append("\nError: ").append(errorText(error));
            }
        }
        if (!currentTitle.isEmpty()) {
            text.append("\nTrack: ").append(currentTitle);
            if (!currentArtist.isEmpty()) text.append("\nArtist: ").append(currentArtist);
        }
        if (!note.isEmpty()) text.append("\n").append(note);
        stateView.setText(text.toString());
    }

    private static String stateName(int state) {
        switch (state) {
            case 0: return "IDLE";
            case 1: return "OPENING";
            case 2: return "PLAYING";
            case 3: return "PAUSED";
            case 4: return "ENDED";
            case 5: return "STOPPED";
            case 6: return "FAILED";
            default: return "?";
        }
    }

    /** Codes match ErrorKind in src/error.rs. */
    private static String errorText(int code) {
        switch (code) {
            case 1: return "Bad file handle";
            case 2: return "File cannot be read";
            case 3: return "Unknown audio format";
            case 4: return "No audio in file";
            case 5: return "Unsupported audio parameters";
            case 6: return "File is damaged";
            case 7: return "Audio output unavailable";
            case 8: return "Audio output refused to start";
            case 9: return "Audio output failed";
            case 10: return "Internal error";
            default: return "Error " + code;
        }
    }

