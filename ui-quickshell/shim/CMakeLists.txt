cmake_minimum_required(VERSION 3.21)
project(retrovert-quickshell LANGUAGES CXX)

# The QML module Quickshell imports as `import Retrovert`. It is a thin scene-graph shim over
# the Rust bridge: every decision about what to draw is made in Rust, this only uploads it.
set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_AUTOMOC ON)

find_package(Qt6 6.5 REQUIRED COMPONENTS Core Gui Qml Quick)
qt_policy(SET QTP0001 NEW)

# The Rust static library and its generated header, built by `cargo build -p retrovert-ui-bridge`.
# RV_BRIDGE_DIR is the cargo target directory for the chosen profile.
set(RV_BRIDGE_DIR "" CACHE PATH "Directory holding libretrovert_ui_bridge.a")
if(NOT RV_BRIDGE_DIR)
    message(FATAL_ERROR "Set -DRV_BRIDGE_DIR to the cargo target dir holding libretrovert_ui_bridge.a")
endif()
add_library(retrovert_ui_bridge STATIC IMPORTED)
set_target_properties(retrovert_ui_bridge PROPERTIES
    IMPORTED_LOCATION "${RV_BRIDGE_DIR}/libretrovert_ui_bridge.a"
    INTERFACE_INCLUDE_DIRECTORIES "${CMAKE_CURRENT_SOURCE_DIR}/../bridge/include")

qt_add_qml_module(retrovert_quickshell
    URI Retrovert
    VERSION 1.0
    PLUGIN_TARGET retrovert_quickshell
    NO_PLUGIN_OPTIONAL
    OUTPUT_DIRECTORY "${CMAKE_BINARY_DIR}/qml/Retrovert"
    SOURCES
        session.h session.cpp
        scope.h scope.cpp
)

target_link_libraries(retrovert_quickshell PRIVATE
    Qt6::Core Qt6::Gui Qt6::Qml Qt6::Quick
    retrovert_ui_bridge
    # What the Rust std links against; a static Rust library does not carry them itself.
    pthread dl m asound)
