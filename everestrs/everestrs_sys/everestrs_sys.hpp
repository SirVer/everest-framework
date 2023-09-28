#pragma once

#include <framework/everest.hpp>
#include <framework/runtime.hpp>
#include <memory>
#include <string>

#include "rust/cxx.h"

struct CommandMeta;
struct JsonBlob;
struct Runtime;

class Module {
public:
    Module(const std::string& module_id, const std::string& prefix, const std::string& conf);

    JsonBlob initialize();
    JsonBlob get_interface(rust::Str interface_name) const;

    void signal_ready(const Runtime& rt) const;
    void provide_command(const Runtime& rt, const CommandMeta& meta) const;
	 void subscribe_variable(const Runtime& rt, const CommandMeta& meta) const;
	 void publish_variable( rust::Str implementation_id, rust::Str name, JsonBlob blob) const;

    // TODO(hrapp): Add call_command and subscribe_variable.

private:
    const std::string module_id_;
    Everest::RuntimeSettings rs_;
    std::unique_ptr<Everest::Config> config_;
    std::unique_ptr<Everest::Everest> handle_;
};

std::unique_ptr<Module> create_module(rust::Str module_name, rust::Str prefix, rust::Str conf);
