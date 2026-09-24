// swift-tools-version: 5.9

import PackageDescription

let package = Package(
    name: "Ailu",
    products: [
        .library(name: "Ailu", targets: ["Ailu"])
    ],
    targets: [
        .target(name: "CAilu"),
        .target(name: "Ailu", dependencies: ["CAilu"])
    ]
)
