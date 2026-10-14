allprojects {
    repositories {
        google()
        mavenCentral()
    }
}

val newBuildDir: Directory =
    rootProject.layout.buildDirectory
        .dir("../../build")
        .get()
rootProject.layout.buildDirectory.value(newBuildDir)

subprojects {
    val newSubprojectBuildDir: Directory = newBuildDir.dir(project.name)
    project.layout.buildDirectory.value(newSubprojectBuildDir)
}
subprojects {
    project.evaluationDependsOn(":app")
}

// Pin every Android subproject (plugins like jni_flutter, rust_builder, …) to
// the Nix-provided NDK and compile SDK, exported as ANDROID_NDK_VERSION and
// ANDROID_COMPILE_SDK, so none of them try to install a version that isn't in
// the read-only Nix store. Both failure modes are real: without the NDK pin,
// plugins pull Flutter's bundled default; without the compile-SDK pin,
// irondash_engine_context (via super_clipboard) asks for platform 31 and the
// build dies with "The SDK directory is not writable". Raising a plugin's
// compileSdk is safe — Android SDKs are backward compatible for compilation.
// Reflection keeps this agnostic to the concrete Android extension type each
// plugin uses (application/library/legacy all differ).
subprojects {
    val ndk = System.getenv("ANDROID_NDK_VERSION")
    val compileSdk = System.getenv("ANDROID_COMPILE_SDK")?.toIntOrNull()
    if (ndk != null || compileSdk != null) {
        val applyPins = {
            extensions.findByName("android")?.let { ext ->
                if (ndk != null) {
                    runCatching {
                        ext.javaClass.getMethod("setNdkVersion", String::class.java)
                            .invoke(ext, ndk)
                    }
                }
                if (compileSdk != null) {
                    runCatching {
                        ext.javaClass.getMethod("setCompileSdkVersion", Int::class.java)
                            .invoke(ext, compileSdk)
                    }
                }
            }
            Unit
        }
        // `evaluationDependsOn(":app")` above forces some projects to evaluate
        // early, so afterEvaluate would throw for them — apply directly if the
        // project is already evaluated, otherwise defer.
        if (state.executed) applyPins() else afterEvaluate { applyPins() }
    }
}

tasks.register<Delete>("clean") {
    delete(rootProject.layout.buildDirectory)
}
