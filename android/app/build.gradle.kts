plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
}

android {
    namespace = "com.ironmind.steelheart"
    compileSdk = 35

    defaultConfig {
        applicationId = "com.ironmind.steelheart"
        // 24 = Android 7.0：wry/tao 的下限，也是 WebView 支持 ES6 的实用下限。
        // 前端一行 babel 都没上，靠的就是这个下限。
        minSdk = 24
        targetSdk = 35
        versionCode = 5
        versionName = "1.4.0"
        // 只出 arm64：现在还用 32 位 arm 的手机跑不动这种活儿，多一个 ABI
        // 就多一份 .so 和一份编译时间。
        ndk { abiFilters += "arm64-v8a" }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
    kotlinOptions { jvmTarget = "17" }

    // wry 的 build.rs 会把 Kotlin 胶水写进这里（见 tools/build-android.ps1
    // 里的 WRY_ANDROID_KOTLIN_FILES_OUT_DIR）。
    sourceSets["main"].kotlin.srcDirs("src/main/kotlin")

    // wry 生成的 Logger.kt 用 `BuildConfig.DEBUG` 决定日志级别。
    // AGP 8 默认不再生成 BuildConfig，必须显式打开。
    buildFeatures { buildConfig = true }

    // 自签名：这个项目不走应用商店，release 包用自己的一张钥匙签。
    //
    // 为什么要一张固定的钥匙（而不是每次现生成）：**换钥匙 = 换签名**，
    // 手机上装更新会被系统拒绝（INSTALL_FAILED_UPDATE_INCOMPATIBLE），
    // 玩家得先卸载、存档（在 app 私有目录里）跟着一起没。
    //
    // 口令不是秘密：这张钥匙只用来"证明是同一个 APK 的后续版本"，
    // 谁都可以重新签一个自己的。真要防篡改得靠商店或 HTTPS 分发。
    signingConfigs {
        create("self") {
            storeFile = file("iron-mind.jks")
            storePassword = "ironmind"
            keyAlias = "ironmind"
            keyPassword = "ironmind"
        }
    }

    buildTypes {
        release {
            // 不混淆：混淆在这里只会让"内核 panic 的栈"变成一堆 a.b.c，
            // 而这个项目最需要的就是能读懂崩溃日志。
            isMinifyEnabled = false
            signingConfig = signingConfigs.getByName("self")
        }
    }

    packaging {
        // .so 已经是编译产物，不需要再压一次
        jniLibs { useLegacyPackaging = false }
    }
}

dependencies {
    // WryActivity 继承 AppCompatActivity，并用这三样做生命周期与返回键：
    implementation("androidx.appcompat:appcompat:1.7.0")
    implementation("androidx.activity:activity-ktx:1.9.3")
    implementation("androidx.lifecycle:lifecycle-process:2.8.7")
    // wry 生成的胶水用到了 WebViewAssetLoader / WebViewCompat / WebViewFeature
    // （文件选择、asset 加载、Some 特性探测）。少了它编译不过——
    // 报错是 `Unresolved reference 'WebViewFeature'`。
    implementation("androidx.webkit:webkit:1.12.1")
    // FileProvider（PermissionHelper 里选图/选文件用）
    implementation("androidx.core:core-ktx:1.13.1")
}
