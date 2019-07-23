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

r#"package org.example;
import android.support.annotation.NonNull;

public final class Boo {

    public Boo() {
        mNativeObj = init();
    }
    private static native long init();

    public final @NonNull String latDirection() {
        return do_latDirection(mNativeObj);
    }
    private static native @NonNull String do_latDirection(long self);"#;

r#"package org.example;
import android.support.annotation.NonNull;

public final class Foo {

    public Foo(double a0, double a1, double a2, double a3, double a4) {
        mNativeObj = init(a0, a1, a2, a3, a4);
    }
    private static native long init(double a0, double a1, double a2, double a3, double a4);

    public final @NonNull Boo dropPoint() {
        long ret = do_dropPoint(mNativeObj);
        Boo convRet = new Boo(InternalPointerMarker.RAW_PTR, ret);

        return convRet;
    }
    private static native long do_dropPoint(long self);"#;
