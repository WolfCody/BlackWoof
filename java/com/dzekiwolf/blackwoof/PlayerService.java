package com.dzekiwolf.blackwoof;

import android.app.Notification;
import android.app.NotificationChannel;
import android.app.NotificationManager;
import android.app.PendingIntent;
import android.app.Service;
import android.content.Intent;
import android.content.pm.ServiceInfo;
import android.media.AudioAttributes;
import android.media.AudioFocusRequest;
import android.media.AudioManager;
import android.media.MediaMetadata;
import android.media.session.MediaSession;
import android.media.session.PlaybackState;
import android.os.Build;
import android.os.Handler;
import android.os.IBinder;
import android.os.Looper;

import java.util.Arrays;

/**
 * Platform glue for background playback: the foreground service, the MediaSession, the media
 * notification and the audio focus request. Rust owns the state and every decision. This class
 * applies the picture Rust sends (show) and forwards what Android reports (focus changes,
 * media buttons, notification buttons) straight to Rust.
 *
 * The Activity starts this service from a user action while it is visible (the Android 15/17
 * rule). START_NOT_STICKY: after the process dies nothing restarts, so nothing plays by itself.
 */
public final class PlayerService extends Service {
    private static final String ACTION_PLAY_PAUSE = "com.dzekiwolf.blackwoof.action.PLAY_PAUSE";
    private static final String ACTION_PREVIOUS = "com.dzekiwolf.blackwoof.action.PREVIOUS";
    private static final String ACTION_NEXT = "com.dzekiwolf.blackwoof.action.NEXT";
    private static final String ACTION_STOP = "com.dzekiwolf.blackwoof.action.STOP";

    private static final String CHANNEL_ID = "playback";
    private static final int NOTIFICATION_ID = 1;

    /** Bits of the actions mask sent by Rust (src/media_session.rs). */
    private static final int BIT_PLAY_PAUSE = 1;
    private static final int BIT_PREVIOUS = 2;
    private static final int BIT_NEXT = 4;

    private static volatile PlayerService instance;

    private Handler handler;
    private MediaSession session;
    private AudioManager audio;
    private AudioFocusRequest focusRequest;

    private int state = PlaybackState.STATE_STOPPED;
    private String title = "";
    private String artist = "";
    private int actions = BIT_PLAY_PAUSE;

    // ---- calls from Rust (any thread); each one hops to the main thread ----------------

    static void show(final int state, final String title, final String artist, final int actions) {
        post(new Task() {
            @Override
            public void run(PlayerService service) {
                service.apply(state, title, artist, actions);
            }
        });
    }

    static void stopService() {
        post(new Task() {
            @Override
            public void run(PlayerService service) {
                service.finish();
            }
        });
    }

    static void requestFocus() {
        post(new Task() {
            @Override
            public void run(PlayerService service) {
                service.requestFocusNow();
            }
        });
    }

    static void abandonFocus() {
        post(new Task() {
            @Override
            public void run(PlayerService service) {
                service.abandonFocusNow();
            }
        });
    }

    private interface Task {
        void run(PlayerService service);
    }

    private static void post(final Task task) {
        final PlayerService service = instance;
        if (service != null) {
            service.handler.post(new Runnable() {
                @Override
                public void run() {
                    task.run(service);
                }
            });
        }
    }

    // ---- service lifecycle ---------------------------------------------------------------

    @Override
    public void onCreate() {
        super.onCreate();
        Platform.init(this);
        handler = new Handler(Looper.getMainLooper());
        audio = getSystemService(AudioManager.class);
        getSystemService(NotificationManager.class).createNotificationChannel(
                new NotificationChannel(CHANNEL_ID, "BLACK WOOF", NotificationManager.IMPORTANCE_LOW));

        session = new MediaSession(this, "BlackWoof");
        session.setCallback(new MediaSession.Callback() {
            @Override
            public void onPlay() {
                command(Native.CMD_PLAY);
            }

            @Override
            public void onPause() {
                command(Native.CMD_PAUSE);
            }

            @Override
            public void onStop() {
                command(Native.CMD_STOP);
            }

            @Override
            public void onSkipToNext() {
                command(Native.CMD_NEXT);
            }

            @Override
            public void onSkipToPrevious() {
                command(Native.CMD_PREVIOUS);
            }
        }, handler);
        session.setActive(true);

        // Satisfies startForegroundService() at once, so a later stop can never race it.
        enterForeground();
        instance = this;
    }

    @Override
    public int onStartCommand(Intent intent, int flags, int startId) {
        String action = intent == null ? null : intent.getAction();
        if (ACTION_STOP.equals(action)) {
            command(Native.CMD_STOP);
            finish();
            return START_NOT_STICKY;
        }
        int command = 0;
        if (ACTION_PLAY_PAUSE.equals(action)) {
            command = Native.CMD_TOGGLE;
        } else if (ACTION_PREVIOUS.equals(action)) {
            command = Native.CMD_PREVIOUS;
        } else if (ACTION_NEXT.equals(action)) {
            command = Native.CMD_NEXT;
        }
        if (command != 0) {
            command(command);
            return START_NOT_STICKY;
        }
        enterForeground();
        // Rust pushes the current picture; if nothing is active there is nothing to keep alive.
        if (!Native.LOADED || Native.nativeServiceReady() == 0) {
            finish();
        }
        return START_NOT_STICKY;
    }

    @Override
    public void onDestroy() {
        instance = null;
        handler.removeCallbacksAndMessages(null);
        if (Native.LOADED) {
            Native.nativeSave();
        }
        abandonFocusNow();
        session.release();
        super.onDestroy();
    }

    @Override
    public IBinder onBind(Intent intent) {
        return null;
    }

    // ---- applying what Rust decided -------------------------------------------------------

    private static void command(int command) {
        if (Native.LOADED) {
            Native.nativeCommand(command);
        }
    }

    private void apply(int newState, String newTitle, String newArtist, int newActions) {
        state = newState;
        title = newTitle == null ? "" : newTitle;
        artist = newArtist == null ? "" : newArtist;
        actions = newActions;

        MediaMetadata.Builder metadata = new MediaMetadata.Builder();
        if (!title.isEmpty()) {
            metadata.putString(MediaMetadata.METADATA_KEY_TITLE, title);
        }
        if (!artist.isEmpty()) {
            metadata.putString(MediaMetadata.METADATA_KEY_ARTIST, artist);
        }
        session.setMetadata(metadata.build());

        long mask = PlaybackState.ACTION_PLAY | PlaybackState.ACTION_PAUSE
                | PlaybackState.ACTION_PLAY_PAUSE | PlaybackState.ACTION_STOP;
        if ((actions & BIT_PREVIOUS) != 0) {
            mask |= PlaybackState.ACTION_SKIP_TO_PREVIOUS;
        }
        if ((actions & BIT_NEXT) != 0) {
            mask |= PlaybackState.ACTION_SKIP_TO_NEXT;
        }
        // No position is sent: the system would otherwise want periodic updates.
        session.setPlaybackState(new PlaybackState.Builder()
                .setActions(mask)
                .setState(state, PlaybackState.PLAYBACK_POSITION_UNKNOWN, 1.0f)
                .build());

        getSystemService(NotificationManager.class).notify(NOTIFICATION_ID, buildNotification());
    }

    private void enterForeground() {
        Notification notification = buildNotification();
        if (Build.VERSION.SDK_INT >= 29) {
            startForeground(NOTIFICATION_ID, notification,
                    ServiceInfo.FOREGROUND_SERVICE_TYPE_MEDIA_PLAYBACK);
        } else {
            startForeground(NOTIFICATION_ID, notification);
        }
    }

    private void finish() {
        stopForeground(Service.STOP_FOREGROUND_REMOVE);
        stopSelf();
    }

    private PendingIntent pending(String action, int requestCode) {
        return PendingIntent.getService(this, requestCode,
                new Intent(this, PlayerService.class).setAction(action),
                PendingIntent.FLAG_IMMUTABLE | PendingIntent.FLAG_UPDATE_CURRENT);
    }

    private Notification buildNotification() {
        PendingIntent open = PendingIntent.getActivity(this, 0,
                new Intent(this, MainActivity.class),
                PendingIntent.FLAG_IMMUTABLE | PendingIntent.FLAG_UPDATE_CURRENT);
        boolean playing = state == PlaybackState.STATE_PLAYING;

        // A notification must have a small icon; the framework's own drawable keeps the
        // notification free of anything we would have to draw. No artwork, ever.
        Notification.Builder builder = new Notification.Builder(this, CHANNEL_ID)
                .setSmallIcon(android.R.drawable.ic_media_play)
                .setContentTitle(title.isEmpty() ? "BLACK WOOF" : title)
                .setContentText(artist)
                .setOngoing(playing)
                .setCategory(Notification.CATEGORY_TRANSPORT)
                .setContentIntent(open);

        int[] compact = new int[3];
        int shown = 0;
        int index = 0;
        if ((actions & BIT_PREVIOUS) != 0) {
            builder.addAction(0, "PREV", pending(ACTION_PREVIOUS, 2));
            compact[shown++] = index++;
        }
        builder.addAction(0, playing ? "PAUSE" : "PLAY", pending(ACTION_PLAY_PAUSE, 3));
        compact[shown++] = index++;
        if ((actions & BIT_NEXT) != 0) {
            builder.addAction(0, "NEXT", pending(ACTION_NEXT, 4));
            compact[shown++] = index++;
        }
        builder.addAction(0, "STOP", pending(ACTION_STOP, 5));

        Notification.MediaStyle style = new Notification.MediaStyle()
                .setMediaSession(session.getSessionToken());
        style.setShowActionsInCompactView(Arrays.copyOf(compact, shown));
        builder.setStyle(style);
        return builder.build();
    }

    // ---- audio focus: Android asks, Rust decides -------------------------------------------

    private void requestFocusNow() {
        if (focusRequest == null) {
            focusRequest = new AudioFocusRequest.Builder(AudioManager.AUDIOFOCUS_GAIN)
                    .setAudioAttributes(new AudioAttributes.Builder()
                            .setUsage(AudioAttributes.USAGE_MEDIA)
                            .setContentType(AudioAttributes.CONTENT_TYPE_MUSIC)
                            .build())
                    .setOnAudioFocusChangeListener(new AudioManager.OnAudioFocusChangeListener() {
                        @Override
                        public void onAudioFocusChange(int change) {
                            if (Native.LOADED) {
                                Native.nativeFocus(change);
                            }
                        }
                    }, handler)
                    .build();
        }
        int result = audio.requestAudioFocus(focusRequest);
        if (result != AudioManager.AUDIOFOCUS_REQUEST_GRANTED && Native.LOADED) {
            Native.nativeFocus(Native.FOCUS_DENIED);
        }
    }

    private void abandonFocusNow() {
        if (focusRequest != null) {
            audio.abandonAudioFocusRequest(focusRequest);
        }
    }
}
