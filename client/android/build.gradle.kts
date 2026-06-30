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
// the Nix-provided NDK exported as ANDROID_NDK_VERSION, so none of them try to
// install Flutter's bundled default into the read-only Nix store. Reflection
// keeps this agnostic to the concrete Android extension type each plugin uses.
subprojects {
    val ndk = System.getenv("ANDROID_NDK_VERSION")
    if (ndk != null) {
        val applyNdk = {
            extensions.findByName("android")?.let { ext ->
                runCatching {
                    ext.javaClass.getMethod("setNdkVersion", String::class.java)
                        .invoke(ext, ndk)
                }
            }
            Unit
        }
        // `evaluationDependsOn(":app")` above forces some projects to evaluate
        // early, so afterEvaluate would throw for them — apply directly if the
        // project is already evaluated, otherwise defer.
        if (state.executed) applyNdk() else afterEvaluate { applyNdk() }
    }
}

tasks.register<Delete>("clean") {
    delete(rootProject.layout.buildDirectory)
}
