#pragma once

#include <niminal/types.hpp>

#include <string>

namespace niminal::app {

constexpr int kJsonEventVersion = 1;

niminal::json json_event(const niminal::StreamEvent& event);
niminal::json session_event(const std::string& type, const std::string& session_id,
                            bool success = true);
niminal::json message_event(const std::string& session_id, const std::string& turn_id,
                            const std::string& role, const std::string& content,
                            const std::string& model = {}, bool final = true);
niminal::json queue_event(const std::string& session_id, const std::string& action, int depth,
                          const std::string& content = {}, const std::string& request_id = {},
                          const std::string& mode = {});

} // namespace niminal::app
