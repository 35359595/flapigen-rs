r##"package org.example;
import android.support.annotation.NonNull;

public final class TrackInfo {

    private TrackInfo() {}

    public final @NonNull java.util.Date start_time() {
        long ret = do_start_time(mNativeObj);
        java.util.Date convRet = new java.util.Date(ret);

        return convRet;
    }
    private static native long do_start_time(long self);

    public final @NonNull java.util.OptionalLong end_time() {
        return do_end_time(mNativeObj);
    }
    private static native @NonNull java.util.OptionalLong do_end_time(long self);"##;
