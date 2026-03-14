#pragma once

#include <atomic>
#include <filesystem>
#include <fstream>
#include <functional>
#include <mutex>
#include <optional>
#include <string>
#include <thread>
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

    private:
        struct PendingInput
        {
            std::string from;
            std::vector<uint8_t> payload;
            bool raw{ false };
        };

        struct StateSnapshot
        {
            std::string title;
            std::string pwd;
            bool hasSelection{ false };
            bool atPrompt{ false };
            size_t tabCount{ 0 };
            size_t activeTab{ 0 };
        };

        void threadMain();
        void handleClient(HANDLE pipe);
        HANDLE createServerPipe() const;

        std::string buildResponse(const std::string& request);
        std::string respondPing() const;
        std::string respondState(std::optional<size_t> tabIndex);
        std::string respondTail(size_t lines);
        std::string respondMsg(std::string_view payload);
        std::string respondListTabs();
        std::string respondNewTab();
        std::string respondCloseTab(std::optional<size_t> index);
        std::string respondSwitchTab(size_t index);
        std::string respondFocus();

        bool enqueueInput(std::string from, std::vector<uint8_t>&& payload, bool raw);
        void drainPendingInputs();

        template<typename TResult>
        TResult runOnUiThread(std::function<TResult()> action) const;
        void runVoidOnUiThread(std::function<void()> action) const;

        StateSnapshot captureState(std::optional<size_t> tabIndex);
        std::string captureTailContent(size_t lines) const;
        std::string captureTabList() const;
        bool inferPromptFromViewport(const std::string& viewport, const std::string& pwd, const std::string& title) const;

        std::optional<winrt::com_ptr<Tab>> getTabImpl(std::optional<size_t> index) const;
        winrt::Microsoft::Terminal::Control::TermControl getActiveControl(std::optional<size_t> index) const;

        bool decodeBase64(std::string_view input, std::vector<uint8_t>& output) const;
        std::string toUtf8(std::wstring_view text) const;
        std::wstring fromUtf8(std::string_view text) const;
        std::string sanitizeSessionName(std::string raw) const;
        std::string trimWhitespace(std::string_view text) const;
        std::string sliceLastLines(std::string_view text, size_t requestedLines) const;

        void appendLogLine(std::string line);
        void ensureDirectories();
        void writeSessionFile();
        void removeSessionFile() noexcept;

        void setWindowFocus() const;

        TerminalPage& _page;
        winrt::Windows::UI::Core::CoreDispatcher _dispatcher{ nullptr };

        std::thread _serverThread;
        std::atomic<bool> _stop{ false };
        // Shared cancellation token: survives ControlPlane destruction to prevent
        // use-after-free in queued UI-thread lambdas (T3 fix).
        std::shared_ptr<std::atomic<bool>> _cancelled{ std::make_shared<std::atomic<bool>>(false) };

        std::mutex _pendingMutex;
        std::vector<PendingInput> _pendingInputs;

        std::mutex _logMutex;
        std::ofstream _logFile;

        std::mutex _pipeMutex;
        HANDLE _currentPipe{ INVALID_HANDLE_VALUE };

        const size_t _pid;
        HWND _hwnd{ nullptr };

        std::filesystem::path _rootDir;
        std::filesystem::path _sessionsDir;
        std::filesystem::path _logsDir;
        std::filesystem::path _sessionFile;
        std::filesystem::path _logFilePath;
        std::string _sessionName;
        std::string _safeSessionName;
        std::string _pipeName;
        std::string _pipePath;

        static constexpr size_t kMaxReadSize = 64 * 1024;
    };
}
