cmake_minimum_required(VERSION 3.22)
project(niminal VERSION 0.1.1 LANGUAGES C CXX)

set(CMAKE_CXX_STANDARD 23)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_CXX_EXTENSIONS OFF)
set(CMAKE_EXPORT_COMPILE_COMMANDS ON)

option(NIMINAL_SANITIZERS "Enable AddressSanitizer and UndefinedBehaviorSanitizer" OFF)

file(MAKE_DIRECTORY "${CMAKE_CURRENT_BINARY_DIR}/generated/include/niminal")
configure_file(
  include/niminal/version.hpp.in
  "${CMAKE_CURRENT_BINARY_DIR}/generated/include/niminal/version.hpp"
  @ONLY)

add_library(niminal_strict_warnings INTERFACE)
target_compile_options(niminal_strict_warnings INTERFACE
  -Wall
  -Wextra
  -Wpedantic
  -Wconversion
  -Wsign-conversion
  -Wshadow
  -Wnon-virtual-dtor
  -Wold-style-cast
  -Werror)

add_library(niminal_sanitizers INTERFACE)
if(NIMINAL_SANITIZERS)
  target_compile_options(niminal_sanitizers INTERFACE
    -fsanitize=address,undefined
    -fno-omit-frame-pointer
    -g)
  target_link_options(niminal_sanitizers INTERFACE
    -fsanitize=address,undefined)
endif()

function(niminal_strict_compile target)
  target_link_libraries(${target} PRIVATE niminal_strict_warnings)
  if(NIMINAL_SANITIZERS)
    target_link_libraries(${target} PRIVATE niminal_sanitizers)
  endif()
endfunction()

if(APPLE AND NOT DEFINED OPENSSL_ROOT_DIR)
  execute_process(
    COMMAND brew --prefix openssl@3
    OUTPUT_VARIABLE OPENSSL_ROOT_DIR
    OUTPUT_STRIP_TRAILING_WHITESPACE
    RESULT_VARIABLE BREW_OPENSSL)
  if(NOT BREW_OPENSSL EQUAL 0)
    execute_process(
      COMMAND brew --prefix openssl
      OUTPUT_VARIABLE OPENSSL_ROOT_DIR
      OUTPUT_STRIP_TRAILING_WHITESPACE)
  endif()
endif()

set(OPENSSL_USE_STATIC_LIBS TRUE)
find_package(OpenSSL REQUIRED)
find_package(ZLIB REQUIRED)

include(FetchContent)

set(JSON_BuildTests OFF CACHE BOOL "" FORCE)
set(JSON_Install OFF CACHE BOOL "" FORCE)
FetchContent_Declare(
  nlohmann_json
  GIT_REPOSITORY https://github.com/nlohmann/json.git
  GIT_TAG v3.11.3
  GIT_SHALLOW TRUE)
FetchContent_MakeAvailable(nlohmann_json)

set(BUILD_SHARED_LIBS OFF CACHE BOOL "" FORCE)
set(BUILD_CURL_EXE OFF CACHE BOOL "" FORCE)
set(BUILD_TESTING OFF CACHE BOOL "" FORCE)
set(BUILD_EXAMPLES OFF CACHE BOOL "" FORCE)
set(BUILD_LIBCURL_DOCS OFF CACHE BOOL "" FORCE)
set(BUILD_MISC_DOCS OFF CACHE BOOL "" FORCE)
set(ENABLE_CURL_MANUAL OFF CACHE BOOL "" FORCE)
set(CURL_DISABLE_INSTALL ON CACHE BOOL "" FORCE)
set(HTTP_ONLY ON CACHE BOOL "" FORCE)
set(CURL_USE_OPENSSL ON CACHE BOOL "" FORCE)
set(CURL_USE_LIBPSL OFF CACHE BOOL "" FORCE)
# The app only uses HTTP; avoid a Homebrew libssh2 runtime dependency.
set(CURL_USE_LIBSSH2 OFF CACHE BOOL "" FORCE)
set(USE_NGHTTP2 OFF CACHE BOOL "" FORCE)
set(USE_LIBIDN2 OFF CACHE BOOL "" FORCE)
set(USE_APPLE_IDN OFF CACHE BOOL "" FORCE)
set(CURL_BROTLI OFF CACHE BOOL "" FORCE)
set(CURL_ZSTD OFF CACHE BOOL "" FORCE)
set(CURL_DISABLE_ALTSVC ON CACHE BOOL "" FORCE)
set(ENABLE_ARES OFF CACHE BOOL "" FORCE)
set(ENABLE_WEBSOCKETS OFF CACHE BOOL "" FORCE)
FetchContent_Declare(
  curl
  GIT_REPOSITORY https://github.com/curl/curl.git
  GIT_TAG curl-8_12_1
  GIT_SHALLOW TRUE)
FetchContent_MakeAvailable(curl)

set(FTXUI_ENABLE_INSTALL OFF CACHE BOOL "" FORCE)
set(FTXUI_BUILD_EXAMPLES OFF CACHE BOOL "" FORCE)
set(FTXUI_BUILD_DOCS OFF CACHE BOOL "" FORCE)
FetchContent_Declare(
  ftxui
  GIT_REPOSITORY https://github.com/ArthurSonzogni/FTXUI.git
  GIT_TAG v7.0.3
  GIT_SHALLOW TRUE)
FetchContent_MakeAvailable(ftxui)

# Some terminals render FTXUI's cursor-shape query instead of treating it as
# a control sequence, briefly exposing "$q" during startup.
set(_ftxui_app "${ftxui_SOURCE_DIR}/src/ftxui/component/app.cpp")
if(EXISTS "${_ftxui_app}")
  file(READ "${_ftxui_app}" _ftxui_app_src)
  string(REPLACE "    TerminalSend(DECRQSS_DECSCUSR);\n" ""
         _ftxui_app_patched "${_ftxui_app_src}")
  if(NOT _ftxui_app_patched STREQUAL _ftxui_app_src)
    file(WRITE "${_ftxui_app}" "${_ftxui_app_patched}")
  endif()
  if(_ftxui_app_patched MATCHES "TerminalSend\\(DECRQSS_DECSCUSR\\)")
    message(FATAL_ERROR "FTXUI cursor-shape query patch was not applied")
  endif()

  # FTXUI drops a selection as soon as a component reports the mouse press as
  # handled. The composer's Input claims every press to place the caret, so its
  # text could not be dragged. Ignore `handled` for mouse events; presses still
  # reach the component, and a press without dragging leaves no selection.
  string(REPLACE
    "bool App::Internal::HandleSelection(bool handled, Event event) {\n  if (handled) {"
    "bool App::Internal::HandleSelection(bool handled, Event event) {\n  if (handled && !event.is_mouse()) {"
    _ftxui_app_selection "${_ftxui_app_patched}")
  if(NOT _ftxui_app_selection STREQUAL _ftxui_app_patched)
    file(WRITE "${_ftxui_app}" "${_ftxui_app_selection}")
  endif()
  if(NOT _ftxui_app_selection MATCHES "handled && !event\\.is_mouse\\(\\)")
    message(FATAL_ERROR "FTXUI composer-selection patch was not applied")
  endif()
endif()

add_library(niminal_ai
  src/niminal/http.cpp
  src/niminal/openai.cpp
  src/niminal/agent.cpp
  src/niminal/providers.cpp)
add_library(niminal::ai ALIAS niminal_ai)

target_include_directories(niminal_ai PUBLIC
  "${CMAKE_CURRENT_BINARY_DIR}/generated/include"
  ${CMAKE_CURRENT_SOURCE_DIR}/include)

target_link_libraries(niminal_ai PUBLIC
  CURL::libcurl
  nlohmann_json::nlohmann_json
  OpenSSL::SSL
  OpenSSL::Crypto
  ZLIB::ZLIB)

if(APPLE)
  target_link_libraries(niminal_ai PUBLIC
    "-framework CoreFoundation"
    "-framework SystemConfiguration")
endif()

niminal_strict_compile(niminal_ai)

add_library(niminal_app
  src/app/tui.cpp
  src/app/clipboard.cpp
  src/app/slash.cpp
  src/app/slash_execute.cpp
  src/app/transcript.cpp
  src/app/session.cpp
  src/app/config.cpp
  src/app/settings.cpp
  src/app/keybindings.cpp
  src/app/auth.cpp
  src/app/provider.cpp
  src/app/instructions.cpp
  src/app/workspace.cpp
  src/app/tools.cpp
  src/app/compaction.cpp
  src/app/markdown.cpp
  src/app/mentions.cpp
  src/app/images.cpp
  src/app/models_dev.cpp
  src/app/thinking.cpp
  src/app/prompts.cpp
  src/app/json_mode.cpp
  src/app/rpc.cpp
  src/app/skills.cpp
  src/app/permissions.cpp
  src/app/trust.cpp
  src/app/extensions.cpp
  src/app/shutdown.cpp
  src/app/diff.cpp
  src/app/theme.cpp)
target_include_directories(niminal_app PUBLIC src/app)
target_link_libraries(niminal_app PUBLIC niminal::ai ftxui::component)
niminal_strict_compile(niminal_app)

add_executable(niminal_cli src/app/main.cpp)
if(APPLE)
  enable_language(OBJCXX)
  target_sources(niminal_cli PRIVATE src/app/clipboard_macos.mm)
  set_source_files_properties(src/app/clipboard_macos.mm PROPERTIES COMPILE_FLAGS "-fobjc-arc")
  target_link_libraries(niminal_cli PRIVATE "-framework AppKit")
endif()
set_target_properties(niminal_cli PROPERTIES OUTPUT_NAME niminal)
target_link_libraries(niminal_cli PRIVATE niminal_app)
niminal_strict_compile(niminal_cli)

function(niminal_add_test name library)
  set(target "niminal_${name}_test")
  add_executable("${target}" "tests/${name}_test.cpp")
  target_link_libraries("${target}" PRIVATE "${library}")
  niminal_strict_compile("${target}")
  add_test(NAME "${name}" COMMAND "${target}" ${ARGN})
endfunction()

enable_testing()
niminal_add_test(workspace niminal_app)
niminal_add_test(mentions niminal_app)
niminal_add_test(json_mode niminal_app)
niminal_add_test(tools niminal_app)
niminal_add_test(instructions niminal_app)
niminal_add_test(skills niminal_app)
niminal_add_test(prompts niminal_app)
niminal_add_test(config niminal_app)
niminal_add_test(keybindings niminal_app)
niminal_add_test(slash niminal_app)
niminal_add_test(settings niminal_app)
niminal_add_test(provider niminal_app)
niminal_add_test(auth niminal_app)
niminal_add_test(session niminal_app)
niminal_add_test(compaction niminal_app)
niminal_add_test(agent niminal::ai)
niminal_add_test(agent_retry niminal::ai)
niminal_add_test(markdown niminal_app)
niminal_add_test(models_dev niminal_app)
niminal_add_test(thinking niminal_app)
niminal_add_test(permissions niminal_app)
niminal_add_test(trust niminal_app)
niminal_add_test(extensions niminal_app)
target_compile_definitions(niminal_extensions_test PRIVATE
  NIMINAL_EXTENSIONS_FIXTURES_DIR="${CMAKE_CURRENT_SOURCE_DIR}/tests/fixtures/extensions")
niminal_add_test(diff niminal_app)
niminal_add_test(theme niminal_app)
niminal_add_test(transcript niminal_app)
niminal_add_test(http niminal::ai)
niminal_add_test(rpc niminal_app)
niminal_add_test(version niminal::ai $<TARGET_FILE:niminal_cli>)
add_dependencies(niminal_version_test niminal_cli)
