package org.chainv4.ui;

import android.Manifest;
import android.app.Activity;
import android.content.Context;
import android.content.pm.PackageManager;
import android.graphics.ImageFormat;
import android.hardware.camera2.CameraAccessException;
import android.hardware.camera2.CameraCaptureSession;
import android.hardware.camera2.CameraCharacteristics;
import android.hardware.camera2.CameraDevice;
import android.hardware.camera2.CameraManager;
import android.hardware.camera2.CaptureRequest;
import android.hardware.camera2.params.StreamConfigurationMap;
import android.media.Image;
import android.media.ImageReader;
import android.os.Handler;
import android.os.HandlerThread;
import android.util.Size;
import android.view.Surface;
import java.nio.ByteBuffer;
import java.util.Collections;

/**
 * The back camera, for reading QR codes live: its frames, in grey, for the app to read and to show
 * itself. It draws nothing: an app that is a NativeActivity draws with egui, which shows the frames
 * as they come. Camera2 hands frames to an ImageReader on a thread of its own; the newest is kept
 * here, its luma plane alone, which is all a QR code needs, until the app takes it.
 *
 * The app asks for the CAMERA permission itself, the first time: Android shows its prompt over the
 * NativeActivity, whose answer the app reads back by asking again whether it was given.
 *
 * chainv4-ui calls these through JNI. ui/build-android.sh compiles this into the APK.
 *
 * No anonymous classes: d8 fails on one nested in another as newer javacs write them.
 */
public final class Camera implements ImageReader.OnImageAvailableListener {
    /** The longest side the frames are asked at: enough for a code that fills a third of them. */
    static final int LONGEST = 1280;

    private static final Object LOCK = new Object();

    /** The camera running, if one is. */
    private static Camera running;

    /** The newest frame, until taken: its luma, its width and height, and how far to turn it upright. */
    private static byte[] luma;
    private static int width;
    private static int height;
    private static int rotation;

    /** Why the camera stopped, until taken. */
    private static String failure;

    private final HandlerThread thread = new HandlerThread("chainv4-camera");
    private Handler handler;
    private CameraDevice device;
    private CameraCaptureSession session;
    private ImageReader reader;
    private int orientation;

    /** Whether the app may use the camera. */
    public static boolean permitted(Activity app) {
        return app.checkSelfPermission(Manifest.permission.CAMERA) == PackageManager.PERMISSION_GRANTED;
    }

    /** Shows Android's prompt for the camera. */
    public static void ask(Activity app) {
        app.requestPermissions(new String[] {Manifest.permission.CAMERA}, 4501);
    }

    /** Opens the back camera, or the first there is, unless one runs already. */
    public static void start(Activity app) {
        synchronized (LOCK) {
            if (running != null) {
                return;
            }

            failure = null;
            luma = null;
            running = new Camera();
        }

        running.open(app);
    }

    /** Closes the camera, if one runs. */
    public static void stop() {
        Camera camera;

        synchronized (LOCK) {
            camera = running;
            running = null;
            luma = null;
        }

        if (camera != null) {
            camera.close();
        }
    }

    /**
     * The newest frame since the last, taken: {byte[] luma, int[] {width, height, rotation}}; or
     * {null, null, String failure} once the camera failed; null when nothing new came.
     */
    public static Object[] frame() {
        synchronized (LOCK) {
            if (failure != null) {
                String why = failure;
                failure = null;

                return new Object[] {null, null, why};
            }

            if (luma == null) {
                return null;
            }

            Object[] taken = new Object[] {luma, new int[] {width, height, rotation}, null};
            luma = null;

            return taken;
        }
    }

    static void fail(String why) {
        synchronized (LOCK) {
            failure = why;
        }

        stop();
    }

    private void open(Activity app) {
        thread.start();
        handler = new Handler(thread.getLooper());

        try {
            CameraManager manager = (CameraManager) app.getSystemService(Context.CAMERA_SERVICE);
            String chosen = null;

            for (String id : manager.getCameraIdList()) {
                Integer facing = manager.getCameraCharacteristics(id).get(CameraCharacteristics.LENS_FACING);

                if (chosen == null || (facing != null && facing == CameraCharacteristics.LENS_FACING_BACK)) {
                    chosen = id;
                }
            }

            if (chosen == null) {
                fail("this device has no camera");

                return;
            }

            CameraCharacteristics characteristics = manager.getCameraCharacteristics(chosen);
            Integer sensor = characteristics.get(CameraCharacteristics.SENSOR_ORIENTATION);
            orientation = sensor == null ? 0 : sensor;
            Size size = size(characteristics.get(CameraCharacteristics.SCALER_STREAM_CONFIGURATION_MAP));
            reader = ImageReader.newInstance(size.getWidth(), size.getHeight(), ImageFormat.YUV_420_888, 2);
            reader.setOnImageAvailableListener(this, handler);
            manager.openCamera(chosen, new Opened(this), handler);
        } catch (CameraAccessException | SecurityException | IllegalArgumentException e) {
            fail("could not open the camera: " + e.getMessage());
        }
    }

    /** The largest size of luma frames no longer than LONGEST on either side. */
    private static Size size(StreamConfigurationMap map) {
        Size best = new Size(640, 480);

        if (map == null) {
            return best;
        }

        for (Size candidate : map.getOutputSizes(ImageFormat.YUV_420_888)) {
            boolean fits = candidate.getWidth() <= LONGEST && candidate.getHeight() <= LONGEST;

            if (fits && candidate.getWidth() * candidate.getHeight() > best.getWidth() * best.getHeight()) {
                best = candidate;
            }
        }

        return best;
    }

    void opened(CameraDevice opened) {
        device = opened;

        try {
            Surface surface = reader.getSurface();
            // The list form: SessionConfiguration, which replaced it, needs Android 9.
            device.createCaptureSession(Collections.singletonList(surface), new Configured(this, surface), handler);
        } catch (CameraAccessException | IllegalStateException e) {
            fail("could not start the camera: " + e.getMessage());
        }
    }

    void configured(CameraCaptureSession configured, Surface surface) {
        session = configured;

        try {
            CaptureRequest.Builder request = device.createCaptureRequest(CameraDevice.TEMPLATE_PREVIEW);
            request.addTarget(surface);
            request.set(CaptureRequest.CONTROL_AF_MODE, CaptureRequest.CONTROL_AF_MODE_CONTINUOUS_PICTURE);
            session.setRepeatingRequest(request.build(), null, handler);
        } catch (CameraAccessException | IllegalStateException e) {
            fail("could not start the camera: " + e.getMessage());
        }
    }

    @Override
    public void onImageAvailable(ImageReader from) {
        Image image = from.acquireLatestImage();

        if (image == null) {
            return;
        }

        try {
            Image.Plane plane = image.getPlanes()[0];
            ByteBuffer buffer = plane.getBuffer();
            int w = image.getWidth();
            int h = image.getHeight();
            int stride = plane.getRowStride();
            byte[] read = new byte[w * h];

            for (int row = 0; row < h; row++) {
                buffer.position(row * stride);
                buffer.get(read, row * w, w);
            }

            synchronized (LOCK) {
                if (running == this) {
                    luma = read;
                    width = w;
                    height = h;
                    rotation = orientation;
                }
            }
        } finally {
            image.close();
        }
    }

    private void close() {
        try {
            if (session != null) {
                session.close();
            }

            if (device != null) {
                device.close();
            }

            if (reader != null) {
                reader.close();
            }
        } finally {
            thread.quitSafely();
        }
    }

    /** Hands the opened camera on, or says why it would not open. */
    static final class Opened extends CameraDevice.StateCallback {
        private final Camera camera;

        Opened(Camera camera) {
            this.camera = camera;
        }

        @Override
        public void onOpened(CameraDevice device) {
            camera.opened(device);
        }

        @Override
        public void onDisconnected(CameraDevice device) {
            device.close();
            fail("the camera went away");
        }

        @Override
        public void onError(CameraDevice device, int error) {
            device.close();
            fail("the camera failed, error " + error);
        }
    }

    /** Starts the frames once the session is set up. */
    static final class Configured extends CameraCaptureSession.StateCallback {
        private final Camera camera;
        private final Surface surface;

        Configured(Camera camera, Surface surface) {
            this.camera = camera;
            this.surface = surface;
        }

        @Override
        public void onConfigured(CameraCaptureSession session) {
            camera.configured(session, surface);
        }

        @Override
        public void onConfigureFailed(CameraCaptureSession session) {
            fail("the camera would not start");
        }
    }
}
