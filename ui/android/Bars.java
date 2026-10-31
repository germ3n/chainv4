package org.chainv4.ui;

import android.app.Activity;
import android.os.Build;
import android.view.View;
import android.view.Window;
import android.view.WindowInsetsController;

/**
 * The system bars' icons, dark on a light page or light on a dark one. The theme in the manifest
 * follows the phone's own setting, not the app's, so a light page under a dark phone's bars had pale
 * icons on it. Windows belong to the main thread, and the app's runs elsewhere.
 */
public final class Bars {
    private Bars() {}

    public static void icons(final Activity activity, final boolean dark) {
        activity.runOnUiThread(new Runnable() {
            @Override
            public void run() {
                Window window = activity.getWindow();

                if (window == null) {
                    return;
                }

                if (Build.VERSION.SDK_INT >= 30) {
                    WindowInsetsController bars = window.getInsetsController();

                    if (bars != null) {
                        int both = WindowInsetsController.APPEARANCE_LIGHT_STATUS_BARS | WindowInsetsController.APPEARANCE_LIGHT_NAVIGATION_BARS;
                        bars.setSystemBarsAppearance(dark ? both : 0, both);
                    }

                    return;
                }

                View decor = window.getDecorView();
                int both = View.SYSTEM_UI_FLAG_LIGHT_STATUS_BAR | (Build.VERSION.SDK_INT >= 26 ? View.SYSTEM_UI_FLAG_LIGHT_NAVIGATION_BAR : 0);
                int kept = decor.getSystemUiVisibility() & ~both;
                decor.setSystemUiVisibility(dark ? kept | both : kept);
            }
        });
    }
}
