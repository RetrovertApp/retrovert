cmake_minimum_required(VERSION 3.16)
project(rv_host C)

add_executable(rv_host rv_host.c)
target_include_directories(rv_host PRIVATE ${CMAKE_CURRENT_SOURCE_DIR}/../abi)
target_compile_features(rv_host PRIVATE c_std_11)

if(NOT WIN32)
    target_link_libraries(rv_host PRIVATE dl m)
endif()
