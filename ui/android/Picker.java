package org.chainv4.ui;

import android.app.Activity;
import android.content.ActivityNotFoundException;
import android.content.ClipData;
import android.content.Intent;
import android.database.Cursor;
import android.graphics.drawable.ColorDrawable;
import android.net.Uri;
import android.os.Bundle;
import android.provider.OpenableColumns;
import java.io.ByteArrayOutputStream;
import java.io.IOException;
import java.io.InputStream;
import java.util.ArrayList;
import java.util.List;

/**
 * Android's document picker, for an app that is a NativeActivity with no Java of its own. Android
 * hands what was picked back to the activity that asked, through onActivityResult, which
 * NativeActivity drops. So the app starts this one instead: it asks for a file or several, reads
 * each whole away from the main thread, leaves what it read for the app to take, and goes. It
 * draws nothing; its window is the app's own background, so the moment it shows does not flash.
 *
 * chainv4-ui calls start and take through JNI. ui/build-android.sh compiles this into the APK.
 *
 * No anonymous classes: d8 fails on one nested in another as newer javacs write them.
 */
public final class Picker extends Activity implements Runnable {
    /** The most it reads of the files picked, all told, so a big one cannot run the app out of memory. */
    static final long MOST = 32L << 20;

    private static final Object LOCK = new Object();

    /** What the last pick came to, until the app takes it: {String[] names, byte[][] contents, String failure}. */
    private static Object[] done;

    /** Starts a pick from the app's activity, for several files or one, over the app's background. */
    public static void start(Activity app, boolean many, int background) {
        synchronized (LOCK) {
            done = null;
        }

        Intent intent = new Intent(app, Picker.class)
                .putExtra("many", many)
                .putExtra("background", background)
                .addFlags(Intent.FLAG_ACTIVITY_NO_ANIMATION);
        app.startActivity(intent);
    }

    /**
     * What the pick came to, taken: names and contents, empty when nothing was picked, or why it
     * failed. Null until then, and once taken.
     */
    public static Object[] take() {
        synchronized (LOCK) {
            Object[] taken = done;
            done = null;

            return taken;
        }
    }

    private static void leave(String[] names, byte[][] contents, String failure) {
        synchronized (LOCK) {
            done = new Object[] {names, contents, failure};
        }
    }

    @Override
    protected void onCreate(Bundle saved) {
        super.onCreate(saved);
        getWindow().setBackgroundDrawable(new ColorDrawable(getIntent().getIntExtra("background", 0xff000000)));

        // Brought back after Android let it go, it has asked already: the answer is on its way.
        if (saved != null) {
            return;
        }

        Intent pick = new Intent(Intent.ACTION_OPEN_DOCUMENT)
                .addCategory(Intent.CATEGORY_OPENABLE)
                .setType("*/*")
                .putExtra(Intent.EXTRA_ALLOW_MULTIPLE, getIntent().getBooleanExtra("many", false));

        try {
            startActivityForResult(pick, 1);
        } catch (ActivityNotFoundException e) {
            leave(null, null, "this device has no document picker");
            close();
        }
    }

    @Override
    protected void onActivityResult(int request, int result, Intent data) {
        final List<Uri> picked = new ArrayList<Uri>();

        if (result == RESULT_OK && data != null) {
            ClipData several = data.getClipData();

            if (several != null) {
                for (int i = 0; i < several.getItemCount(); i++) {
                    picked.add(several.getItemAt(i).getUri());
                }
            } else if (data.getData() != null) {
                picked.add(data.getData());
            }
        }

        // Reading a big file would hold up the main thread. Android lets the app read what was
        // picked for as long as this activity lives, so it goes only once all is read.
        new Thread(new Reading(this, picked), "chainv4 picker").start();
    }

    /** Goes, on the main thread, once what was picked is read. */
    @Override
    public void run() {
        close();
    }

    /** Reads what was picked, then has the picker go. */
    private static final class Reading implements Runnable {
        private final Picker picker;
        private final List<Uri> picked;

        Reading(Picker picker, List<Uri> picked) {
            this.picker = picker;
            this.picked = picked;
        }

        @Override
        public void run() {
            picker.read(picked);
            picker.runOnUiThread(picker);
        }
    }

    private void read(List<Uri> picked) {
        String[] names = new String[picked.size()];
        byte[][] contents = new byte[picked.size()][];
        long room = MOST;

        try {
            for (int i = 0; i < picked.size(); i++) {
                names[i] = name(picked.get(i));
                contents[i] = bytes(picked.get(i), room);
                room -= contents[i].length;
            }

            leave(names, contents, null);
        } catch (IOException | RuntimeException e) {
            leave(null, null, e.getMessage() != null ? e.getMessage() : e.toString());
        }
    }

    /** The file's name, as the app that holds it says, or the last part of its address. */
    private String name(Uri uri) {
        Cursor cursor = getContentResolver().query(uri, new String[] {OpenableColumns.DISPLAY_NAME}, null, null, null);

        if (cursor != null) {
            try {
                if (cursor.moveToFirst() && !cursor.isNull(0)) {
                    return cursor.getString(0);
                }
            } finally {
                cursor.close();
            }
        }

        String last = uri.getLastPathSegment();

        return last != null ? last : "file";
    }

    /** Reads a file whole, failing once it holds more than room. */
    private byte[] bytes(Uri uri, long room) throws IOException {
        InputStream in = getContentResolver().openInputStream(uri);

        if (in == null) {
            throw new IOException("cannot open " + uri);
        }

        try {
            ByteArrayOutputStream out = new ByteArrayOutputStream();
            byte[] buffer = new byte[64 * 1024];

            for (int read; (read = in.read(buffer)) != -1; ) {
                out.write(buffer, 0, read);

                if (out.size() > room) {
                    throw new IOException("the files picked come to more than " + (MOST >> 20) + " MiB");
                }
            }

            return out.toByteArray();
        } finally {
            in.close();
        }
    }

    private void close() {
        finish();
        overridePendingTransition(0, 0);
    }
}
