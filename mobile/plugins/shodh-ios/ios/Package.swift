// swift-tools-version:5.5

// Shodh's iOS glue, built as a static library and linked into the app by the
// plugin's build script. The package is named after the crate: the build
// links the library product of that name (tauri-utils' link_apple_library).
// Tauri's Swift API is copied to ../.tauri/tauri-api by the same build.

import PackageDescription

let package = Package(
  name: "tauri-plugin-shodh-ios",
  platforms: [
    .macOS(.v10_15),
    .iOS(.v14),
  ],
  products: [
    .library(
      name: "tauri-plugin-shodh-ios",
      type: .static,
      targets: ["tauri-plugin-shodh-ios"])
  ],
  dependencies: [
    .package(name: "Tauri", path: "../.tauri/tauri-api")
  ],
  targets: [
    .target(
      name: "tauri-plugin-shodh-ios",
      dependencies: [
        .byName(name: "Tauri")
      ],
      path: "Sources")
  ]
)
