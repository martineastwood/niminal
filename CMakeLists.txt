cmake_minimum_required(VERSION 3.22)
project(niminal VERSION 0.1.0 LANGUAGES C CXX)

set(CMAKE_CXX_STANDARD 23)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_CXX_EXTENSIONS OFF)
set(CMAKE_EXPORT_COMPILE_COMMANDS ON)

option(NIMINAL_SANITIZERS "Enable AddressSanitizer and UndefinedBehaviorSanitizer" OFF)

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
  GIT_TAG v7.0.1
  GIT_SHALLOW TRUE)
FetchContent_MakeAvailable(ftxui)

add_library(niminal_ai
  src/niminal/http.cpp
  src/niminal/openai.cpp
  src/niminal/agent.cpp
  src/niminal/providers.cpp)
add_library(niminal::ai ALIAS niminal_ai)

target_include_directories(niminal_ai PUBLIC
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

add_executable(niminal_cli
  src/app/main.cpp
  src/app/tui.cpp
  src/app/session.cpp
  src/app/config.cpp
  src/app/provider.cpp
  src/app/instructions.cpp
  src/app/workspace.cpp
  src/app/tools.cpp
  src/app/compaction.cpp
  src/app/markdown.cpp
  src/app/mentions.cpp
  src/app/models_dev.cpp
  src/app/thinking.cpp
  src/app/prompts.cpp
  src/app/json_mode.cpp
  src/app/rpc.cpp)
target_sources(niminal_cli PRIVATE src/app/skills.cpp)
target_sources(niminal_cli PRIVATE src/app/permissions.cpp src/app/trust.cpp)
target_sources(niminal_cli PRIVATE src/app/extensions.cpp)
target_sources(niminal_cli PRIVATE src/app/diff.cpp)
target_sources(niminal_cli PRIVATE src/app/theme.cpp)
target_sources(niminal_cli PRIVATE src/app/rpc_protocol.cpp)
set_target_properties(niminal_cli PROPERTIES OUTPUT_NAME niminal)
target_include_directories(niminal_cli PRIVATE src/app)
target_link_libraries(niminal_cli PRIVATE
  niminal::ai
  ftxui::component)
niminal_strict_compile(niminal_cli)

add_executable(niminal_workspace_test tests/workspace_test.cpp src/app/workspace.cpp)
target_include_directories(niminal_workspace_test PRIVATE src/app)
niminal_strict_compile(niminal_workspace_test)

add_executable(niminal_mentions_test tests/mentions_test.cpp src/app/mentions.cpp src/app/workspace.cpp)
target_include_directories(niminal_mentions_test PRIVATE src/app include)
niminal_strict_compile(niminal_mentions_test)

add_executable(niminal_json_mode_test tests/json_mode_test.cpp src/app/json_mode.cpp)
target_include_directories(niminal_json_mode_test PRIVATE src/app include)
target_link_libraries(niminal_json_mode_test PRIVATE nlohmann_json::nlohmann_json)
niminal_strict_compile(niminal_json_mode_test)

add_executable(niminal_tools_test
  tests/tools_test.cpp
  src/app/tools.cpp
  src/app/workspace.cpp
  src/app/instructions.cpp
  src/app/config.cpp)
target_include_directories(niminal_tools_test PRIVATE src/app include)
target_link_libraries(niminal_tools_test PRIVATE niminal::ai nlohmann_json::nlohmann_json)
niminal_strict_compile(niminal_tools_test)

add_executable(niminal_instructions_test
  tests/instructions_test.cpp
  src/app/instructions.cpp
  src/app/config.cpp)
target_include_directories(niminal_instructions_test PRIVATE src/app include)
target_link_libraries(niminal_instructions_test PRIVATE niminal::ai nlohmann_json::nlohmann_json)
niminal_strict_compile(niminal_instructions_test)

add_executable(niminal_skills_test tests/skills_test.cpp src/app/skills.cpp src/app/config.cpp
  src/app/trust.cpp)
target_include_directories(niminal_skills_test PRIVATE src/app include)
target_link_libraries(niminal_skills_test PRIVATE niminal::ai nlohmann_json::nlohmann_json)
niminal_strict_compile(niminal_skills_test)

add_executable(niminal_prompts_test tests/prompts_test.cpp src/app/prompts.cpp src/app/config.cpp
  src/app/trust.cpp)
target_include_directories(niminal_prompts_test PRIVATE src/app include)
target_link_libraries(niminal_prompts_test PRIVATE niminal::ai nlohmann_json::nlohmann_json)
niminal_strict_compile(niminal_prompts_test)

add_executable(niminal_config_test tests/config_test.cpp src/app/config.cpp)
target_include_directories(niminal_config_test PRIVATE src/app)
target_link_libraries(niminal_config_test PRIVATE niminal::ai nlohmann_json::nlohmann_json)
niminal_strict_compile(niminal_config_test)

add_executable(niminal_provider_test
  tests/provider_test.cpp
  src/app/provider.cpp
  src/app/config.cpp
  src/app/thinking.cpp
  src/app/models_dev.cpp)
target_include_directories(niminal_provider_test PRIVATE src/app include)
target_link_libraries(niminal_provider_test PRIVATE niminal::ai)
niminal_strict_compile(niminal_provider_test)

add_executable(niminal_session_test tests/session_test.cpp src/app/session.cpp)
target_include_directories(niminal_session_test PRIVATE src/app include)
target_link_libraries(niminal_session_test PRIVATE nlohmann_json::nlohmann_json)
niminal_strict_compile(niminal_session_test)

add_executable(niminal_compaction_test
  tests/compaction_test.cpp
  src/app/session.cpp
  src/app/compaction.cpp
  src/app/extensions.cpp
  src/app/config.cpp
  src/app/trust.cpp)
target_include_directories(niminal_compaction_test PRIVATE src/app include)
target_link_libraries(niminal_compaction_test PRIVATE niminal::ai)
niminal_strict_compile(niminal_compaction_test)

add_executable(niminal_agent_test tests/agent_test.cpp)
target_link_libraries(niminal_agent_test PRIVATE niminal::ai)
niminal_strict_compile(niminal_agent_test)

add_executable(niminal_agent_retry_test tests/agent_retry_test.cpp)
target_link_libraries(niminal_agent_retry_test PRIVATE niminal::ai)
niminal_strict_compile(niminal_agent_retry_test)

add_executable(niminal_markdown_test tests/markdown_test.cpp src/app/markdown.cpp)
target_include_directories(niminal_markdown_test PRIVATE src/app)
target_link_libraries(niminal_markdown_test PRIVATE ftxui::dom)
niminal_strict_compile(niminal_markdown_test)

add_executable(niminal_models_dev_test
  tests/models_dev_test.cpp
  src/app/models_dev.cpp
  src/app/config.cpp)
target_include_directories(niminal_models_dev_test PRIVATE src/app include)
target_link_libraries(niminal_models_dev_test PRIVATE niminal::ai)
niminal_strict_compile(niminal_models_dev_test)

add_executable(niminal_thinking_test
  tests/thinking_test.cpp
  src/app/thinking.cpp
  src/app/models_dev.cpp
  src/app/config.cpp)
target_include_directories(niminal_thinking_test PRIVATE src/app include)
target_link_libraries(niminal_thinking_test PRIVATE niminal::ai)
niminal_strict_compile(niminal_thinking_test)

add_executable(niminal_permissions_test
  tests/permissions_test.cpp
  src/app/permissions.cpp
  src/app/trust.cpp)
target_include_directories(niminal_permissions_test PRIVATE src/app include)
target_link_libraries(niminal_permissions_test PRIVATE nlohmann_json::nlohmann_json)
niminal_strict_compile(niminal_permissions_test)

add_executable(niminal_trust_test
  tests/trust_test.cpp
  src/app/trust.cpp)
target_include_directories(niminal_trust_test PRIVATE src/app)
target_link_libraries(niminal_trust_test PRIVATE nlohmann_json::nlohmann_json)
niminal_strict_compile(niminal_trust_test)

add_executable(niminal_extensions_test
  tests/extensions_test.cpp
  src/app/extensions.cpp
  src/app/session.cpp
  src/app/compaction.cpp
  src/app/config.cpp
  src/app/trust.cpp)
target_include_directories(niminal_extensions_test PRIVATE src/app include)
target_link_libraries(niminal_extensions_test PRIVATE niminal::ai)
niminal_strict_compile(niminal_extensions_test)

add_executable(niminal_diff_test tests/diff_test.cpp src/app/diff.cpp)
target_include_directories(niminal_diff_test PRIVATE src/app)
target_link_libraries(niminal_diff_test PRIVATE nlohmann_json::nlohmann_json)
niminal_strict_compile(niminal_diff_test)

add_executable(niminal_theme_test tests/theme_test.cpp src/app/theme.cpp)
target_include_directories(niminal_theme_test PRIVATE src/app)
target_link_libraries(niminal_theme_test PRIVATE ftxui::dom)
niminal_strict_compile(niminal_theme_test)

add_executable(niminal_http_test tests/http_test.cpp)
target_link_libraries(niminal_http_test PRIVATE niminal::ai)
niminal_strict_compile(niminal_http_test)

add_executable(niminal_rpc_test tests/rpc_test.cpp src/app/rpc_protocol.cpp)
target_include_directories(niminal_rpc_test PRIVATE src/app include)
target_link_libraries(niminal_rpc_test PRIVATE nlohmann_json::nlohmann_json)
niminal_strict_compile(niminal_rpc_test)

enable_testing()
add_test(NAME workspace COMMAND niminal_workspace_test)
add_test(NAME mentions COMMAND niminal_mentions_test)
add_test(NAME json_mode COMMAND niminal_json_mode_test)
add_test(NAME tools COMMAND niminal_tools_test)
add_test(NAME instructions COMMAND niminal_instructions_test)
add_test(NAME skills COMMAND niminal_skills_test)
add_test(NAME prompts COMMAND niminal_prompts_test)
add_test(NAME config COMMAND niminal_config_test)
add_test(NAME provider COMMAND niminal_provider_test)
add_test(NAME session COMMAND niminal_session_test)
add_test(NAME compaction COMMAND niminal_compaction_test)
add_test(NAME agent COMMAND niminal_agent_test)
add_test(NAME agent_retry COMMAND niminal_agent_retry_test)
add_test(NAME markdown COMMAND niminal_markdown_test)
add_test(NAME models_dev COMMAND niminal_models_dev_test)
add_test(NAME thinking COMMAND niminal_thinking_test)
add_test(NAME permissions COMMAND niminal_permissions_test)
add_test(NAME trust COMMAND niminal_trust_test)
add_test(NAME extensions COMMAND niminal_extensions_test)
add_test(NAME diff COMMAND niminal_diff_test)
add_test(NAME theme COMMAND niminal_theme_test)
add_test(NAME http COMMAND niminal_http_test)
add_test(NAME rpc COMMAND niminal_rpc_test)
