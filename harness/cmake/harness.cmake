# Injected into a plugin's configure via -DCMAKE_PROJECT_INCLUDE so the
# harness — not the plugin repo — owns the release floor: symbol hiding,
# static runtimes, arch baseline, and reproducible-output flags. Plugins
# build unmodified.

set(HARNESS_CMAKE_DIR "${CMAKE_CURRENT_LIST_DIR}")

set(CMAKE_C_VISIBILITY_PRESET hidden)
set(CMAKE_CXX_VISIBILITY_PRESET hidden)
set(CMAKE_VISIBILITY_INLINES_HIDDEN ON)
set(CMAKE_POSITION_INDEPENDENT_CODE ON)

if(WIN32)
    # Static release CRT; compiler-runtime DLL imports are forbidden by the
    # import audit.
    set(CMAKE_MSVC_RUNTIME_LIBRARY "MultiThreaded")
    string(APPEND CMAKE_SHARED_LINKER_FLAGS " /Brepro")
    string(APPEND CMAKE_EXE_LINKER_FLAGS " /Brepro")
else()
    string(APPEND CMAKE_SHARED_LINKER_FLAGS
        " -Wl,--version-script=${HARNESS_CMAKE_DIR}/exports.map"
        " -Wl,--exclude-libs,ALL"
        " -Wl,-z,defs"
        " -static-libstdc++ -static-libgcc")
    foreach(lang C CXX)
        string(APPEND CMAKE_${lang}_FLAGS
            " -ffile-prefix-map=${CMAKE_SOURCE_DIR}=/rvsrc"
            " -ffile-prefix-map=${CMAKE_BINARY_DIR}=/rvbuild"
            " -Werror=date-time")
    endforeach()
    if(CMAKE_SYSTEM_PROCESSOR MATCHES "aarch64|arm64")
        string(APPEND CMAKE_C_FLAGS " -march=armv8-a")
        string(APPEND CMAKE_CXX_FLAGS " -march=armv8-a")
    endif()
endif()
