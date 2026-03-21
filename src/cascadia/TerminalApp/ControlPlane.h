#pragma once

#include <atomic>
#include <chrono>
#include <filesystem>
#include <fstream>
#include <functional>
#include <mutex>
#include <optional>
#include <string>
#include <thread>
#include <unordered_map>
#include <vector>

#include "TerminalPage.h"
#include "Tab.h"

namespace winrt::TerminalApp::implementation
{
    struct ControlPlane final
    {
    public:
        static bool IsEnabled();

        explicit ControlPlane(TerminalPage& page);
        ~ControlPlane();

        ControlPlane(const ControlPlane&) = delete;
        ControlPlane& operator=(const ControlPlane&) = delete;

        // ──── Public methods called from VTable callbacks ────
        std::string captureTailContent(size_t lines) const;
        std::string captureTailContentForTab(size_t tabIndex) const;
        bool enqueueInput(std::string from, std::vector<uint8_t>&& payload, bool raw);
        void drainPendingInputs();
        void sendInputToTab(std::vector<uint8_t> payload, bool raw, size_t tabIndex);
        size_t captureTabCount();
        size_t captureActiveTab();
        void doSwitchTab(size_t index);
        void doNewTab();
        void doCloseTab(size_t index);
        void doFocus();
        HWND getHwnd() const;
        std::string captureTabTitle(size_t index);
        std::string captureTabWorkingDir(size_t index);
        bool captureTabHasSelection(size_t index);

    private:
        struct PendingInput
        {
            std::string from;
            std::vector<uint8_t> payload;
            bool raw{ false };
        };

        void initDll();
        void appendDiagLog(const std::string& message);

        template<typename TResult>
        TResult runOnUiThread(std::function<TResult()> action) const;
        void runVoidOnUiThread(std::function<void()> action) const;

        std::optional<winrt::com_ptr<Tab>> getTabImpl(std::optional<size_t> index) const;
        winrt::Microsoft::Terminal::Control::TermControl getActiveControl(std::optional<size_t> index) const;

        std::string toUtf8(std::wstring_view text) const;
        std::wstring fromUtf8(std::string_view text) const;
        std::string sanitizeSessionName(std::string raw) const;

        TerminalPage& _page;
        winrt::Windows::UI::Core::CoreDispatcher _dispatcher{ nullptr };

        std::atomic<bool> _stop{ false };
        std::shared_ptr<std::atomic<bool>> _cancelled{ std::make_shared<std::atomic<bool>>(false) };

        std::mutex _pendingMutex;
        std::vector<PendingInput> _pendingInputs;

        const size_t _pid;
        HWND _hwnd{ nullptr };

        std::string _sessionName;
        std::string _safeSessionName;
        std::string _pipeName;
        std::string _pipePath;

        // DLL runtime loading
        HMODULE _dllHandle{ nullptr };
        void* _dllServer{ nullptr };
        void  (*_fnStop)(void*){ nullptr };
        void  (*_fnDestroy)(void*){ nullptr };
    };
}
