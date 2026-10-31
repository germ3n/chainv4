package org.chainv4.ui;

import android.app.Activity;
import android.content.DialogInterface;
import android.hardware.biometrics.BiometricManager;
import android.hardware.biometrics.BiometricPrompt;
import android.hardware.fingerprint.FingerprintManager;
import android.os.Build;
import android.os.CancellationSignal;
import android.security.keystore.KeyGenParameterSpec;
import android.security.keystore.KeyPermanentlyInvalidatedException;
import android.security.keystore.KeyProperties;
import android.security.keystore.KeyInfo;
import java.security.KeyStore;
import java.util.Arrays;
import javax.crypto.Cipher;
import javax.crypto.KeyGenerator;
import javax.crypto.SecretKey;
import javax.crypto.SecretKeyFactory;
import javax.crypto.spec.GCMParameterSpec;

/**
 * Biometrics for an app that is a NativeActivity: a secret wrapped by an AES key in the Android
 * Keystore that opens only after a strong biometric check, which BiometricPrompt makes. A finger or
 * face enrolled after the key was made invalidates it for good, so someone who adds their own
 * cannot open what was kept. A key is only made inside secure hardware: a wallet may depend on it
 * alone, and says that a copy of its storage opens nothing.
 *
 * chainv4-ui calls available, start, take and forget through JNI. start shows the prompt on the
 * main thread and returns; what came of it waits for take, as the picker's does. ui/build-android.sh
 * compiles this into the APK with the picker.
 *
 * The secret passes through here: what is handed in to keep is overwritten once it is sealed, or
 * the prompt turned down, and what an opening gives back is overwritten by chainv4-ui once read.
 *
 * BiometricPrompt is Android 9's (API 28); before that, available says there are none.
 *
 * No anonymous classes: d8 fails on one nested in another as newer javacs write them. So this one
 * class is the callback, the runnable that shows the prompt, and the listener of its button.
 */
public final class Biometric extends BiometricPrompt.AuthenticationCallback implements Runnable, DialogInterface.OnClickListener {
    /** Keep data under a new key: what comes back is it sealed, and the IV. */
    static final int ENROLL = 0;
    /** Open data sealed under a key: what comes back is it open. */
    static final int OPEN = 1;

    private static final String STORE = "AndroidKeyStore";
    private static final Object LOCK = new Object();

    /** What the last check came to, until the app takes it: {byte[] out, byte[] iv, String failure}. */
    private static Object[] done;

    private final Activity activity;
    private final Cipher cipher;
    private final int mode;
    private final byte[] data;
    private final String title;
    private final String subtitle;

    private Biometric(Activity activity, Cipher cipher, int mode, byte[] data, String title, String subtitle) {
        this.activity = activity;
        this.cipher = cipher;
        this.mode = mode;
        this.data = data;
        this.title = title;
        this.subtitle = subtitle;
    }

    /** 2 when strong biometrics can be used, 1 when the phone has them but none is enrolled, 0 when not. */
    @SuppressWarnings("deprecation")
    public static int available(Activity activity) {
        if (Build.VERSION.SDK_INT < 28) {
            return 0;
        }

        if (Build.VERSION.SDK_INT >= 29) {
            BiometricManager manager = activity.getSystemService(BiometricManager.class);

            if (manager == null) {
                return 0;
            }

            int can = Build.VERSION.SDK_INT >= 30 ? manager.canAuthenticate(BiometricManager.Authenticators.BIOMETRIC_STRONG) : manager.canAuthenticate();

            if (can == BiometricManager.BIOMETRIC_SUCCESS) {
                return 2;
            }

            return can == BiometricManager.BIOMETRIC_ERROR_NONE_ENROLLED ? 1 : 0;
        }

        // Android 9 has no BiometricManager: the fingerprint is the strong biometric there.
        FingerprintManager fingerprint = activity.getSystemService(FingerprintManager.class);

        if (fingerprint == null || !fingerprint.isHardwareDetected()) {
            return 0;
        }

        return fingerprint.hasEnrolledFingerprints() ? 2 : 1;
    }

    /**
     * Shows the prompt to keep data under a new key, or to open data sealed under one. A key that a
     * new enrollment invalidated, or that is gone, comes to the failure "invalidated" at once.
     */
    public static void start(Activity activity, String alias, int mode, byte[] data, byte[] iv, String title, String subtitle) {
        synchronized (LOCK) {
            done = null;
        }

        try {
            Cipher cipher = Cipher.getInstance("AES/GCM/NoPadding");

            if (mode == ENROLL) {
                SecretKey key = create(alias);

                if (!inSecureHardware(key)) {
                    forget(alias);
                    wipe(mode, data);
                    finish(null, null, "this phone's Keystore is not in secure hardware, so a wallet cannot depend on it");

                    return;
                }

                cipher.init(Cipher.ENCRYPT_MODE, key);
            } else {
                SecretKey key = existing(alias);

                if (key == null) {
                    finish(null, null, "invalidated");

                    return;
                }

                cipher.init(Cipher.DECRYPT_MODE, key, new GCMParameterSpec(128, iv));
            }

            activity.runOnUiThread(new Biometric(activity, cipher, mode, data, title, subtitle));
        } catch (KeyPermanentlyInvalidatedException err) {
            wipe(mode, data);
            forget(alias);
            finish(null, null, "invalidated");
        } catch (Exception err) {
            wipe(mode, data);
            finish(null, null, err.toString());
        }
    }

    /** Overwrites the secret handed in to keep, once it is sealed or will not be. */
    private static void wipe(int mode, byte[] data) {
        if (mode == ENROLL && data != null) {
            Arrays.fill(data, (byte) 0);
        }
    }

    /** What the check came to, taken: {out, iv, failure}. Null until then, and once taken. */
    public static Object[] take() {
        synchronized (LOCK) {
            Object[] taken = done;
            done = null;

            return taken;
        }
    }

    /** Deletes a key, and with it whatever it sealed. */
    public static void forget(String alias) {
        try {
            KeyStore store = KeyStore.getInstance(STORE);
            store.load(null);
            store.deleteEntry(alias);
        } catch (Exception err) {
            // Gone already, or no store: nothing is left to open either way.
        }
    }

    /** A new AES key, usable only right after a strong biometric check, gone with a new enrollment. */
    private static SecretKey create(String alias) throws Exception {
        KeyGenParameterSpec.Builder spec = new KeyGenParameterSpec.Builder(alias, KeyProperties.PURPOSE_ENCRYPT | KeyProperties.PURPOSE_DECRYPT)
                .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
                .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
                .setKeySize(256)
                .setUserAuthenticationRequired(true)
                .setInvalidatedByBiometricEnrollment(true);

        if (Build.VERSION.SDK_INT >= 30) {
            spec.setUserAuthenticationParameters(0, KeyProperties.AUTH_BIOMETRIC_STRONG);
        }

        KeyGenerator generator = KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, STORE);
        generator.init(spec.build());

        return generator.generateKey();
    }

    /** Whether a key is kept in the phone's TEE or StrongBox, rather than by software. */
    @SuppressWarnings("deprecation")
    private static boolean inSecureHardware(SecretKey key) throws Exception {
        SecretKeyFactory factory = SecretKeyFactory.getInstance(key.getAlgorithm(), STORE);
        KeyInfo info = (KeyInfo) factory.getKeySpec(key, KeyInfo.class);

        return info.isInsideSecureHardware();
    }

    private static SecretKey existing(String alias) throws Exception {
        KeyStore store = KeyStore.getInstance(STORE);
        store.load(null);

        return (SecretKey) store.getKey(alias, null);
    }

    private static void finish(byte[] out, byte[] iv, String failure) {
        synchronized (LOCK) {
            done = new Object[] {out, iv, failure};
        }
    }

    /** Shows the prompt, on the main thread. */
    @Override
    public void run() {
        try {
            BiometricPrompt.Builder builder = new BiometricPrompt.Builder(activity)
                    .setTitle(title)
                    .setNegativeButton("Cancel", activity.getMainExecutor(), this);

            if (subtitle != null && !subtitle.isEmpty()) {
                builder.setSubtitle(subtitle);
            }

            if (Build.VERSION.SDK_INT >= 30) {
                builder.setAllowedAuthenticators(BiometricManager.Authenticators.BIOMETRIC_STRONG);
            }

            builder.build().authenticate(new BiometricPrompt.CryptoObject(cipher), new CancellationSignal(), activity.getMainExecutor(), this);
        } catch (Exception err) {
            wipe(mode, data);
            finish(null, null, err.toString());
        }
    }

    @Override
    public void onAuthenticationSucceeded(BiometricPrompt.AuthenticationResult result) {
        try {
            Cipher opened = result.getCryptoObject().getCipher();
            byte[] out = opened.doFinal(data);
            wipe(mode, data);
            finish(out, mode == ENROLL ? opened.getIV() : null, null);
        } catch (Exception err) {
            wipe(mode, data);
            finish(null, null, err.toString());
        }
    }

    @Override
    public void onAuthenticationError(int code, CharSequence message) {
        boolean canceled = code == BiometricPrompt.BIOMETRIC_ERROR_USER_CANCELED || code == BiometricPrompt.BIOMETRIC_ERROR_CANCELED;
        wipe(mode, data);
        finish(null, null, canceled ? "canceled" : String.valueOf(message));
    }

    /** "Cancel". */
    @Override
    public void onClick(DialogInterface dialog, int which) {
        wipe(mode, data);
        finish(null, null, "canceled");
    }
}
