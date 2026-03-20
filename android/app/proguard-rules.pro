# ── JNI bridge — ProxyService ─────────────────────────────────────────────────
# R8 must never rename or remove ProxyService or any of its members.
# The Rust .so calls back into Kotlin via:
#   env.find_class("io/github/SafeDNS/ProxyService")
#   env.call_static_method(..., "nativeLog", ...)
# and Kotlin declares `external fun` methods whose JNI names are derived from
# the fully-qualified class name.  Any renaming breaks the link at runtime.
-keep class io.github.SafeDNS.ProxyService { *; }
-keepclassmembers class io.github.SafeDNS.ProxyService {
    # All external (native) functions — Kotlin → Rust calls
    native <methods>;
    # nativeLog — Rust → Kotlin callback
    public static void nativeLog(java.lang.String, java.lang.String, java.lang.String);
    # isVpnActive — read by MainActivity poll loop
    public static boolean isVpnActive;
}

# ── Android components ────────────────────────────────────────────────────────
# Android instantiates these via reflection from AndroidManifest.xml.
# R8 must keep their names and lifecycle method signatures intact.
-keep public class io.github.SafeDNS.MainActivity        { *; }
-keep public class io.github.SafeDNS.ProxyService        { *; }
-keep public class io.github.SafeDNS.ProxyTileService    { *; }
-keep public class io.github.SafeDNS.BootReceiver        { *; }

# ── rustls-platform-verifier ──────────────────────────────────────────────────
# The Rust TLS stack calls into this Java helper at runtime via JNI.
-keep                       class org.rustls.platformverifier.** { *; }
-keep             interface org.rustls.platformverifier.** { *; }
-keep,includedescriptorclasses class org.rustls.platformverifier.** { *; }

# ── Kotlin & Coroutines ───────────────────────────────────────────────────────
# Keep coroutine internal machinery that R8 otherwise aggressively inlines away,
# causing CoroutineExceptionHandler / SupervisorJob crashes in release builds.
-keepnames class kotlinx.coroutines.internal.MainDispatcherFactory { *; }
-keepnames class kotlinx.coroutines.CoroutineExceptionHandler      { *; }
-keep class kotlin.coroutines.Continuation

# ── Compose ───────────────────────────────────────────────────────────────────
# Compose uses reflection and code generation; let the BOM's own consumer rules
# handle most of it, but keep the entry-point Activity name stable.
-keep class androidx.compose.** { *; }
-dontwarn androidx.compose.**

# ── Log stripping ─────────────────────────────────────────────────────────────
# Remove debug and verbose Android log calls in release builds.
# Safe because these have no side-effects beyond the log output itself.
-assumenosideeffects class android.util.Log {
    public static int d(...);
    public static int v(...);
}
