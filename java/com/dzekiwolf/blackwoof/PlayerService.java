package com.dzekiwolf.blackwoof;

import android.app.Notification;
import android.app.NotificationChannel;
import android.app.NotificationManager;
import android.app.PendingIntent;
import android.app.Service;
import android.content.Intent;
import android.content.pm.ServiceInfo;
import android.os.Build;
import android.os.Handler;
import android.os.IBinder;
import android.os.Looper;

/**
 * Minimal platform glue. Android requires a Service subclass and a notification for a
 * foreground service; nothing else lives here. Rust owns the playback state and the decision
 * when the service should end: it calls onEngineState (via JNI, see android_bridge.rs) when a
 * track reaches a final state, and nativeIsActive tells a freshly started service whether
 * there is anything to keep alive.
 *
 * The Activity starts this service from a user action while it is visible (the Android 15/17
 * rule). START_NOT_STICKY: after the process dies nothing restarts, so nothing plays by itself.
 */
public final class PlayerService extends Service {
    static final String ACTION_STOP = "com.dzekiwolf.blackwoof.action.STOP";

    private static final String CHANNEL_ID = "playback";
    private static final int NOTIFICATION_ID = 1;

    private static volatile PlayerService instance;

    private Handler handler;

    /**
     * Called from native code on the playback thread when the engine reaches a final state.
     * Rust decides that a call is needed; this only hops to the main thread and ends the
     * service. packed (state and error) is reserved for the media notification of a later step.
     */
    static void onEngineState(int packed) {
        final PlayerService service = instance;
        if (service != null) {
            service.handler.post(new Runnable() {
                @Override
                public void run() {
                    service.finish();
                }
            });
        }
    }

    @Override
    public void onCreate() {
        super.onCreate();
        handler = new Handler(Looper.getMainLooper());
        NotificationManager manager = getSystemService(NotificationManager.class);
        manager.createNotificationChannel(new NotificationChannel(
                CHANNEL_ID, "BLACK WOOF", NotificationManager.IMPORTANCE_LOW));
        // Satisfies startForegroundService() at once, so a later stop can never race it.
        enterForeground();
        instance = this;
    }

    @Override
    public int onStartCommand(Intent intent, int flags, int startId) {
        if (intent != null && ACTION_STOP.equals(intent.getAction())) {
            if (Native.LOADED) {
                Native.nativeStop();
            }
            finish();
            return START_NOT_STICKY;
        }
        enterForeground();
        // The track may have ended or failed before this service got going.
        if (!Native.LOADED || Native.nativeIsActive() == 0) {
            finish();
        }
        return START_NOT_STICKY;
    }

    @Override
    public void onDestroy() {
        instance = null;
        handler.removeCallbacksAndMessages(null);
        super.onDestroy();
    }

    @Override
    public IBinder onBind(Intent intent) {
        return null;
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

    private Notification buildNotification() {
        PendingIntent open = PendingIntent.getActivity(this, 0,
                new Intent(this, MainActivity.class),
                PendingIntent.FLAG_IMMUTABLE | PendingIntent.FLAG_UPDATE_CURRENT);
        PendingIntent stop = PendingIntent.getService(this, 1,
                new Intent(this, PlayerService.class).setAction(ACTION_STOP),
                PendingIntent.FLAG_IMMUTABLE | PendingIntent.FLAG_UPDATE_CURRENT);
        // A notification must have a small icon; the framework's own drawable keeps the APK
        // free of image resources.
        return new Notification.Builder(this, CHANNEL_ID)
                .setSmallIcon(android.R.drawable.ic_media_play)
                .setContentTitle("BLACK WOOF")
                .setOngoing(true)
                .setCategory(Notification.CATEGORY_TRANSPORT)
                .setContentIntent(open)
                .addAction(0, "STOP", stop)
                .build();
    }
}
