#pragma once

#include <glaze/glaze.hpp>

#include <concepts>
#include <cstddef>
#include <cstdint>
#include <initializer_list>
#include <istream>
#include <optional>
#include <sstream>
#include <stdexcept>
#include <string>
#include <string_view>
#include <utility>

namespace niminal {

using json = glz::generic;

inline json json_object() {
  json value;
  value = json::object_t{};
  return value;
}

inline json json_array(std::initializer_list<json> values = {}) {
  json::array_t array(values.begin(), values.end());
  json value;
  value = std::move(array);
  return value;
}

inline std::string json_dump(const json& value) {
  struct WriteOptions : glz::opts {
    bool escape_control_characters = true;
  };
  auto result = glz::write<WriteOptions{}>(value);
  if (!result) {
    throw std::runtime_error(glz::format_error(result.error()));
  }
  return std::move(*result);
}

inline std::string json_pretty(const json& value) {
  struct PrettyOptions : glz::opts {
    std::uint8_t indentation_width = 2;
    bool escape_control_characters = true;
    constexpr PrettyOptions() : glz::opts{} { prettify = true; }
  };
  auto result = glz::write<PrettyOptions{}>(value);
  if (!result) {
    throw std::runtime_error(glz::format_error(result.error()));
  }
  return std::move(*result);
}

inline json json_parse(std::string_view source) {
  json value;
  if (const auto error = glz::read_json(value, source); error) {
    throw std::runtime_error(glz::format_error(error, source));
  }
  return value;
}

inline json json_parse(std::istream& source) {
  std::ostringstream text;
  text << source.rdbuf();
  return json_parse(text.str());
}

inline std::optional<json> try_json_parse(std::string_view source) noexcept {
  json value;
  if (glz::read_json(value, source)) {
    return std::nullopt;
  }
  return value;
}

inline bool json_equal(const json& left, const json& right) {
  if (left.is_null() || right.is_null()) {
    return left.is_null() && right.is_null();
  }
  if (left.is_boolean() || right.is_boolean()) {
    return left.is_boolean() && right.is_boolean() && left.get<bool>() == right.get<bool>();
  }
  if (left.is_number() || right.is_number()) {
    return left.is_number() && right.is_number() && left.as<double>() == right.as<double>();
  }
  if (left.is_string() || right.is_string()) {
    return left.is_string() && right.is_string() &&
           left.get<std::string>() == right.get<std::string>();
  }
  if (left.is_array() || right.is_array()) {
    if (!left.is_array() || !right.is_array() || left.size() != right.size()) {
      return false;
    }
    for (std::size_t i = 0; i < left.size(); ++i) {
      if (!json_equal(left.get_array()[i], right.get_array()[i])) {
        return false;
      }
    }
    return true;
  }
  if (!left.is_object() || !right.is_object() || left.size() != right.size()) {
    return false;
  }
  for (const auto& [key, value] : left.get_object()) {
    if (!right.contains(key) || !json_equal(value, right[key])) {
      return false;
    }
  }
  return true;
}

template <typename T> T json_as(const json& value) {
  auto result = glz::read_json<T>(value);
  if (!result) {
    throw std::runtime_error(glz::format_error(result.error(), value));
  }
  return std::move(*result);
}

template <typename T> T json_value(const json& object, std::string_view key, const T& fallback) {
  if (!object.is_object() || !object.contains(key)) {
    return fallback;
  }
  if constexpr (std::same_as<T, json>) {
    return object[key];
  } else {
    auto result = glz::read_json<T>(object[key]);
    return result ? std::move(*result) : fallback;
  }
}

template <std::size_t N>
std::string json_value(const json& object, std::string_view key, const char (&fallback)[N]) {
  return json_value(object, key, std::string(fallback));
}

} // namespace niminal
