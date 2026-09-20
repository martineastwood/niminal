#pragma once

#include <niminal/types.hpp>

#include <string>

namespace niminal::app {

nlohmann::json json_event(const niminal::StreamEvent& event);
nlohmann::json session_event(const std::string& type, const std::string& session_id,
                             bool success = true);
nlohmann::json message_event(const std::string& session_id,
                            const std::string& turn_id, const std::string& role,
                            const std::string& content, const std::string& model = {},
                            bool final = true);
nlohmann::json queue_event(const std::string& session_id, const std::string& action,
                           int depth, const std::string& content = {},
                           const std::string& request_id = {},
                           const std::string& mode = {});
nlohmann::json diagnostic_event(const std::string& level,
                               const std::string& message);

}  // namespace niminal::app
