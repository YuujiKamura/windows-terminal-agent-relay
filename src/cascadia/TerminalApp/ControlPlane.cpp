#include "pch.h"
#include "ControlPlane.h"

#include <algorithm>
#include <array>
#include <chrono>
#include <cctype>
#include <cstdlib>
#include <filesystem>
#include <future>
#include <iomanip>
#include <sstream>

// Use projected types only - do NOT include TermControl impl header
// (it causes WinRT vtable errors when included from TerminalApp project)

using namespace winrt::TerminalApp::implementation;
using namespace winrt::Microsoft::Terminal::Control;
using namespace winrt::Microsoft::Terminal::Settings::Model;
using namespace winrt::Windows::UI::Core;
using namespace winrt::Windows::Foundation;

namespace
{
    constexpr std::array<char, 64> kBase64Alphabet{
        'A','B','C','D','E','F','G','H','I','J','K','L','M','N','O','P',
        'Q','R','S','T','U','V','W','X','Y','Z','a','b','c','d','e','f',
        'g','h','i','j','k','l','m','n','o','p','q','r','s','t','u','v',
        'w','x','y','z','0','1','2','3','4','5','6','7','8','9','+','/'};

    constexpr std::array<int, 256> makeBase64Map()
    {
        std::array<int, 256> map;
        map.fill(-1);
        for (size_t i = 0; i < kBase64Alphabet.size(); ++i)
        {
            map[static_cast<unsigned char>(kBase64Alphabet[i])] = static_cast<int>(i);
        }
        map[static_cast<unsigned char>('=')] = -2;
        return map;
    }

    constexpr auto kBase64Map = makeBase64Map();

    bool isTruthy(std::string_view value)
    {
        std::string lower;
        lower.reserve(value.size());
        for (auto ch : value)
        {
            lower.push_back(static_cast<char>(std::tolower(static_cast<unsigned char>(ch))));
        }
        return lower == "1" || lower == "true" || lower == "yes";
    }

    std::string escapeField(std::string_view value)
    {
        std::string out;
        out.reserve(value.size());
        for (auto ch : value)
        {
            if (ch == '|' || ch == '\r' || ch == '\n')
            {
                out.push_back(' ');
            }
            else
            {
                out.push_back(ch);
            }
        }
        return out;
    }

    std::string toHex(uintptr_t value)
    {
        std::ostringstream oss;
        oss << "0x" << std::hex << std::uppercase << value;
        return oss.str();
    }

    std::optional<std::string> getEnvVar(const char* name)
    {
        char* value = nullptr;
        size_t len = 0;
        if (_dupenv_s(&value, &len, name) != 0 || !value)
        {
            return std::nullopt;
        }

        std::string result{ value };
        free(value);
        return result;
    }
}

bool ControlPlane::IsEnabled()
{
    if (const auto flag = getEnvVar("GHOSTTY_CONTROL_PLANE"))
    {
        if (isTruthy(*flag))
        {
            return true;
        }
    }
    if (const auto flag = getEnvVar("GHOSTTY_WIN32_CONTROL_PLANE"))
    {
        if (isTruthy(*flag))
        {
            return true;
        }
    }
    return false;
}

ControlPlane::ControlPlane(TerminalPage& page) :
    _page(page),
    _dispatcher(_page.Dispatcher()),
    _pid(static_cast<size_t>(GetCurrentProcessId()))
{
    if (!_dispatcher)
    {
        return;
    }

    _sessionName = getEnvVar("GHOSTTY_SESSION_NAME").value_or("");
    if (_sessionName.empty())
    {
        _sessionName = "winui3-" + std::to_string(_pid);
    }
    _safeSessionName = sanitizeSessionName(_sessionName);
    _pipeName = "ghostty-winui3-" + _safeSessionName + "-" + std::to_string(_pid);
    _pipePath = "\\\\.\\pipe\\" + _pipeName;
    _hwnd = _page.HostingWindow().value_or(nullptr);

    try
    {
        ensureDirectories();
        writeSessionFile();
        appendLogLine("control-plane-started");
        _logFile.open(_logFilePath, std::ios::app);
        _serverThread = std::thread([this]() { threadMain(); });
    }
    catch (const std::exception& e)
    {
        TraceLoggingWrite(g_hTerminalAppProvider,
                          "ControlPlaneInitFailed",
                          TraceLoggingString(e.what(), "Reason"));
    }
}

ControlPlane::~ControlPlane()
{
    _stop.store(true);
    {
        std::lock_guard<std::mutex> guard(_pipeMutex);
        if (_currentPipe != INVALID_HANDLE_VALUE)
        {
            CancelIoEx(_currentPipe, nullptr);
            FlushFileBuffers(_currentPipe);
        }
    }
    if (_serverThread.joinable())
    {
        _serverThread.join();
    }
    removeSessionFile();
}

void ControlPlane::ensureDirectories()
{
    const auto localApp = getEnvVar("LOCALAPPDATA");
    if (!localApp)
    {
        throw std::runtime_error("LOCALAPPDATA is missing");
    }
    std::filesystem::path root(fromUtf8(*localApp));
    root /= L"ghostty";
    root /= L"control-plane";
    root /= L"winui3";
    _rootDir = root;
    _sessionsDir = _rootDir / L"sessions";
    _logsDir = _rootDir / L"logs";
    std::filesystem::create_directories(_sessionsDir);
    std::filesystem::create_directories(_logsDir);
}

void ControlPlane::writeSessionFile()
{
    const auto baseName = _safeSessionName + "-" + std::to_string(_pid);
    _sessionFile = _sessionsDir / (baseName + ".session");
    _logFilePath = _logsDir / (baseName + ".log");

    std::ofstream session(_sessionFile, std::ios::trunc);
    session << "session_name=" << _sessionName << "\n";
    session << "safe_session_name=" << _safeSessionName << "\n";
    session << "pid=" << _pid << "\n";
    session << "hwnd=" << toHex(reinterpret_cast<uintptr_t>(_hwnd)) << "\n";
    session << "pipe_name=" << _pipeName << "\n";
    session << "pipe_path=" << _pipePath << "\n";
    session << "log_file=" << toUtf8(_logFilePath.wstring()) << "\n";
}

void ControlPlane::removeSessionFile() noexcept
{
    std::error_code ec;
    std::filesystem::remove(_sessionFile, ec);
}

void ControlPlane::appendLogLine(std::string line)
{
    std::lock_guard<std::mutex> guard(_logMutex);
    if (_logFile.is_open())
    {
        _logFile << line << "\n";
        _logFile.flush();
    }
}

HANDLE ControlPlane::createServerPipe() const
{
    return CreateNamedPipeA(
        _pipePath.c_str(),
        PIPE_ACCESS_DUPLEX,
        PIPE_TYPE_BYTE | PIPE_WAIT,
        1,
        static_cast<DWORD>(kMaxReadSize),
        static_cast<DWORD>(kMaxReadSize),
        0,
        nullptr);
}

void ControlPlane::threadMain()
{
    while (!_stop.load())
    {
        const auto pipe = createServerPipe();
        if (pipe == INVALID_HANDLE_VALUE)
        {
            break;
        }
        {
            std::lock_guard<std::mutex> guard(_pipeMutex);
            _currentPipe = pipe;
        }
        const auto connected = ConnectNamedPipe(pipe, nullptr) ? true : (GetLastError() == ERROR_PIPE_CONNECTED);
        if (connected)
        {
            handleClient(pipe);
        }
        DisconnectNamedPipe(pipe);
        CloseHandle(pipe);
        {
            std::lock_guard<std::mutex> guard(_pipeMutex);
            _currentPipe = INVALID_HANDLE_VALUE;
        }
    }
}

void ControlPlane::handleClient(HANDLE pipe)
{
    std::vector<char> buffer(kMaxReadSize);
    DWORD read = 0;
    if (!ReadFile(pipe, buffer.data(), static_cast<DWORD>(buffer.size()), &read, nullptr))
    {
        return;
    }
    const std::string request(buffer.data(), read);
    const auto trimmed = trimWhitespace(request);
    if (trimmed.empty())
    {
        return;
    }
    const auto response = buildResponse(trimmed);
    DWORD written = 0;
    WriteFile(pipe, response.data(), static_cast<DWORD>(response.size()), &written, nullptr);
    FlushFileBuffers(pipe);
}

std::string ControlPlane::buildResponse(const std::string& request)
{
    if (request == "PING")
    {
        return respondPing();
    }
    if (request == "STATE" || request.rfind("STATE|", 0) == 0)
    {
        std::optional<size_t> tabIdx;
        if (request.rfind("STATE|", 0) == 0)
        {
            const auto arg = request.substr(6);
            if (!arg.empty())
            {
                tabIdx = static_cast<size_t>(std::stoull(arg));
            }
        }
        return respondState(tabIdx);
    }
    if (request == "TAIL" || request.rfind("TAIL|", 0) == 0)
    {
        size_t lines = 20;
        if (request.rfind("TAIL|", 0) == 0)
        {
            const auto arg = request.substr(5);
            if (!arg.empty())
            {
                lines = static_cast<size_t>(std::stoull(arg));
            }
        }
        return respondTail(lines);
    }
    if (request == "LIST_TABS")
    {
        return respondListTabs();
    }
    if (request.rfind("MSG|", 0) == 0)
    {
        return respondMsg(request.substr(4));
    }
    if (request.rfind("INPUT|", 0) == 0 || request.rfind("RAW_INPUT|", 0) == 0)
    {
        const bool raw = request.rfind("RAW_INPUT|", 0) == 0;
        const auto payload = request.substr(raw ? 10 : 6);
        const auto sep = payload.find('|');
        if (sep == std::string::npos)
        {
            return "ERR|" + _sessionName + "|invalid-input\n";
        }
        const auto from = payload.substr(0, sep);
        const auto encoded = payload.substr(sep + 1);
        std::vector<uint8_t> decoded;
        if (!decodeBase64(encoded, decoded))
        {
            return "ERR|" + _sessionName + "|invalid-base64\n";
        }
        const auto decodedSize = decoded.size();
        if (!enqueueInput(std::string(from), std::move(decoded), raw))
        {
            return "ERR|" + _sessionName + "|enqueue-failed\n";
        }
        drainPendingInputs();
        appendLogLine(std::string(raw ? "RAW_INPUT" : "INPUT") + "|" + std::string(from) + "|" + std::to_string(decodedSize));
        return "ACK|" + _sessionName + "|" + std::to_string(_pid) + "\n";
    }
    if (request == "NEW_TAB")
    {
        return respondNewTab();
    }
    if (request == "CLOSE_TAB" || request.rfind("CLOSE_TAB|", 0) == 0)
    {
        std::optional<size_t> idx;
        if (request.rfind("CLOSE_TAB|", 0) == 0)
        {
            const auto arg = request.substr(10);
            if (!arg.empty())
            {
                idx = static_cast<size_t>(std::stoull(arg));
            }
        }
        return respondCloseTab(idx);
    }
    if (request.rfind("SWITCH_TAB|", 0) == 0)
    {
        const auto arg = request.substr(11);
        if (arg.empty())
        {
            return "ERR|" + _sessionName + "|missing-tab-index\n";
        }
        return respondSwitchTab(static_cast<size_t>(std::stoull(arg)));
    }
    if (request == "FOCUS")
    {
        return respondFocus();
    }
    return "ERR|" + _sessionName + "|unknown\n";
}

std::string ControlPlane::respondPing() const
{
    std::ostringstream oss;
    oss << "PONG|" << _sessionName << "|" << _pid << "|" << toHex(reinterpret_cast<uintptr_t>(_hwnd)) << "\n";
    return oss.str();
}

std::string ControlPlane::respondState(std::optional<size_t> tabIndex)
{
    const auto snapshot = captureState(tabIndex);
    std::ostringstream oss;
    oss << "STATE|" << _sessionName << "|" << _pid << "|" << toHex(reinterpret_cast<uintptr_t>(_hwnd)) << "|";
    oss << escapeField(toUtf8(_page.Title())) << "|prompt=" << (snapshot.atPrompt ? '1' : '0') << "|selection=" << (snapshot.hasSelection ? '1' : '0');
    oss << "|pwd=" << snapshot.pwd << "|tab_count=" << snapshot.tabCount << "|active_tab=" << snapshot.activeTab << "\n";
    return oss.str();
}

std::string ControlPlane::respondTail(size_t lines)
{
    const auto data = captureTailContent(lines);
    const auto sliced = sliceLastLines(data, lines);
    std::ostringstream oss;
    oss << "TAIL|" << _sessionName << "|" << lines << "\n" << sliced;
    if (!sliced.empty() && sliced.back() != '\n')
    {
        oss << '\n';
    }
    return oss.str();
}

std::string ControlPlane::respondListTabs()
{
    return captureTabList();
}

std::string ControlPlane::respondMsg(std::string_view payload)
{
    appendLogLine("MSG|" + std::string(payload));
    return "ACK|" + _sessionName + "|" + std::to_string(_pid) + "\n";
}

std::string ControlPlane::respondNewTab()
{
    runVoidOnUiThread([this]() {
        _page._OpenNewTab(nullptr);
    });
    appendLogLine("NEW_TAB");
    return "ACK|" + _sessionName + "|NEW_TAB\n";
}

std::string ControlPlane::respondCloseTab(std::optional<size_t> index)
{
    const auto idx = index.value_or(0);
    runVoidOnUiThread([this, idx]() {
        const auto tabCount = _page._tabs.Size();
        if (tabCount > 0 && idx < tabCount)
        {
            _page._CloseTabAtIndex(static_cast<uint32_t>(idx));
        }
    });
    appendLogLine("CLOSE_TAB|" + std::to_string(idx));
    return "ACK|" + _sessionName + "|CLOSE_TAB|" + std::to_string(idx) + "\n";
}

std::string ControlPlane::respondSwitchTab(size_t index)
{
    runVoidOnUiThread([this, index]() {
        _page._SelectTab(static_cast<uint32_t>(index));
    });
    appendLogLine("SWITCH_TAB|" + std::to_string(index));
    return "ACK|" + _sessionName + "|SWITCH_TAB|" + std::to_string(index) + "\n";
}

std::string ControlPlane::respondFocus()
{
    setWindowFocus();
    appendLogLine("FOCUS");
    return "ACK|" + _sessionName + "|FOCUS\n";
}

bool ControlPlane::enqueueInput(std::string from, std::vector<uint8_t>&& payload, bool raw)
{
    try
    {
        std::lock_guard<std::mutex> guard(_pendingMutex);
        _pendingInputs.push_back(PendingInput{ std::move(from), std::move(payload), raw });
        return true;
    }
    catch (...)
    {
        return false;
    }
}

void ControlPlane::drainPendingInputs()
{
    std::vector<PendingInput> pending;
    {
        std::lock_guard<std::mutex> guard(_pendingMutex);
        pending.swap(_pendingInputs);
    }
    if (pending.empty())
    {
        return;
    }
    runVoidOnUiThread([this, inputs = std::move(pending)]() {
        for (const auto& entry : inputs)
        {
            const auto control = getActiveControl(std::nullopt);
            if (!control)
            {
                continue;
            }
            const auto text = fromUtf8(std::string_view(
                reinterpret_cast<const char*>(entry.payload.data()),
                entry.payload.size()));
            if (entry.raw)
            {
                // RAW_INPUT: write directly to connection (bypass paste encoder)
                control.SendInput(winrt::hstring(text));
            }
            else
            {
                // INPUT: send as paste-style input
                control.SendInput(winrt::hstring(text));
            }
        }
    });
}

ControlPlane::StateSnapshot ControlPlane::captureState(std::optional<size_t> tabIndex)
{
    return runOnUiThread<StateSnapshot>([this, tabIndex]() {
        StateSnapshot snapshot{};
        snapshot.tabCount = _page.NumberOfTabs();
        snapshot.activeTab = _page._GetFocusedTabIndex().value_or(0);
        if (const auto tab = getTabImpl(tabIndex))
        {
            const auto control = tab.value()->GetActiveTerminalControl();
            snapshot.hasSelection = control.HasSelection();
            snapshot.pwd = toUtf8(control.WorkingDirectory());
            // ViewportText is impl-only; use title for prompt heuristic
            snapshot.atPrompt = inferPromptFromViewport("", snapshot.pwd, toUtf8(tab.value()->Title()));
        }
        return snapshot;
    });
}

std::string ControlPlane::captureTailContent(size_t) const
{
    // ViewportText requires TermControl impl access which is unavailable
    // from TerminalApp project. Return empty for now.
    // TODO: expose ViewportText via IDL to enable TAIL support.
    return std::string();
}

std::string ControlPlane::captureTabList() const
{
    return runOnUiThread<std::string>([this]() {
        std::ostringstream oss;
        const auto tabCount = _page.NumberOfTabs();
        const auto focused = _page._GetFocusedTabIndex().value_or(0);
        oss << "LIST_TABS|" << tabCount << "|" << focused << "\n";
        for (uint32_t i = 0; i < tabCount; ++i)
        {
            const auto tab = _page._tabs.GetAt(i);
            if (auto tabImpl = _page._GetTabImpl(tab))
            {
                const auto control = tabImpl->GetActiveTerminalControl();
                const auto title = escapeField(toUtf8(tabImpl->Title()));
                const auto pwd = toUtf8(control.WorkingDirectory());
                const auto prompt = inferPromptFromViewport("", pwd, title);
                oss << "TAB|" << i << "|" << title << "|pwd=" << pwd;
                oss << "|prompt=" << (prompt ? '1' : '0') << "|selection=" << (control.HasSelection() ? '1' : '0') << "\n";
            }
        }
        return oss.str();
    });
}

bool ControlPlane::inferPromptFromViewport(const std::string& viewport, const std::string& pwd, const std::string&) const
{
    const auto trimmed = trimWhitespace(viewport);
    if (trimmed.empty())
    {
        return false;
    }
    const auto lastLine = sliceLastLines(trimmed, 1);
    if (lastLine.empty())
    {
        return false;
    }
    const char lastChar = lastLine.back();
    if (lastChar == '>' || lastChar == '$' || lastChar == '#')
    {
        return true;
    }
    if (!pwd.empty() && lastLine.rfind(pwd, 0) == 0)
    {
        return true;
    }
    return false;
}

std::optional<winrt::com_ptr<Tab>> ControlPlane::getTabImpl(std::optional<size_t> index) const
{
    if (_page._tabs.Size() == 0)
    {
        return std::nullopt;
    }
    size_t idx = index.value_or(_page._GetFocusedTabIndex().value_or(0));
    idx = std::min<size_t>(idx, std::max<size_t>(1, _page._tabs.Size()) - 1);
    auto tab = _page._tabs.GetAt(static_cast<uint32_t>(idx));
    return _page._GetTabImpl(tab);
}

winrt::Microsoft::Terminal::Control::TermControl ControlPlane::getActiveControl(std::optional<size_t> index) const
{
    if (const auto tab = getTabImpl(index))
    {
        return tab.value()->GetActiveTerminalControl();
    }
    return nullptr;
}

bool ControlPlane::decodeBase64(std::string_view input, std::vector<uint8_t>& output) const
{
    output.clear();
    int bits = 0;
    uint32_t value = 0;
    for (auto ch : input)
    {
        if (std::isspace(static_cast<unsigned char>(ch)))
        {
            continue;
        }
        const int decoded = kBase64Map[static_cast<unsigned char>(ch)];
        if (decoded == -1)
        {
            return false;
        }
        if (decoded == -2)
        {
            break;
        }
        value = (value << 6) | decoded;
        bits += 6;
        if (bits >= 8)
        {
            bits -= 8;
            output.push_back(static_cast<uint8_t>((value >> bits) & 0xFF));
        }
    }
    return true;
}

std::string ControlPlane::toUtf8(std::wstring_view text) const
{
    if (text.empty())
    {
        return {};
    }
    const auto size = WideCharToMultiByte(CP_UTF8, 0, text.data(), static_cast<int>(text.size()), nullptr, 0, nullptr, nullptr);
    if (size <= 0)
    {
        return {};
    }
    std::string output(size, '\0');
    WideCharToMultiByte(CP_UTF8, 0, text.data(), static_cast<int>(text.size()), output.data(), size, nullptr, nullptr);
    return output;
}

std::wstring ControlPlane::fromUtf8(std::string_view text) const
{
    if (text.empty())
    {
        return {};
    }
    const auto size = MultiByteToWideChar(CP_UTF8, 0, text.data(), static_cast<int>(text.size()), nullptr, 0);
    if (size <= 0)
    {
        return {};
    }
    std::wstring output(size, '\0');
    MultiByteToWideChar(CP_UTF8, 0, text.data(), static_cast<int>(text.size()), output.data(), size);
    return output;
}

std::string ControlPlane::sanitizeSessionName(std::string raw) const
{
    std::string sanitized;
    sanitized.reserve(raw.size());
    for (auto ch : raw)
    {
        if (std::isalnum(static_cast<unsigned char>(ch)) || ch == '-' || ch == '_' || ch == '.')
        {
            sanitized.push_back(ch);
        }
        else
        {
            sanitized.push_back('_');
        }
    }
    while (!sanitized.empty() && sanitized.front() == '_')
    {
        sanitized.erase(sanitized.begin());
    }
    while (!sanitized.empty() && sanitized.back() == '_')
    {
        sanitized.pop_back();
    }
    if (sanitized.empty())
    {
        sanitized = "session";
    }
    return sanitized;
}

std::string ControlPlane::trimWhitespace(std::string_view text) const
{
    size_t start = 0;
    while (start < text.size() && std::isspace(static_cast<unsigned char>(text[start])))
    {
        ++start;
    }
    size_t end = text.size();
    while (end > start && std::isspace(static_cast<unsigned char>(text[end - 1])))
    {
        --end;
    }
    return std::string(text.substr(start, end - start));
}

std::string ControlPlane::sliceLastLines(std::string_view text, size_t requestedLines) const
{
    if (text.empty() || requestedLines == 0)
    {
        return std::string(text);
    }
    size_t seen = 0;
    size_t idx = text.size();
    while (idx > 0)
    {
        --idx;
        if (text[idx] == '\n')
        {
            ++seen;
            if (seen > requestedLines)
            {
                return std::string(text.substr(idx + 1));
            }
        }
    }
    return std::string(text);
}

void ControlPlane::setWindowFocus() const
{
    if (_hwnd)
    {
        SetForegroundWindow(_hwnd);
    }
}

template<typename TResult>
TResult ControlPlane::runOnUiThread(std::function<TResult()> action) const
{
    std::promise<TResult> promise;
    auto future = promise.get_future();
    _dispatcher.RunAsync(CoreDispatcherPriority::Normal, [action = std::move(action), promise = std::move(promise)]() mutable {
        try
        {
            promise.set_value(action());
        }
        catch (...)
        {
            promise.set_exception(std::current_exception());
        }
    });
    return future.get();
}

// Helper: run void action on UI thread (avoids template<void> specialization issues)
void ControlPlane::runVoidOnUiThread(std::function<void()> action) const
{
    std::promise<void> promise;
    auto future = promise.get_future();
    _dispatcher.RunAsync(CoreDispatcherPriority::Normal, [action = std::move(action), promise = std::move(promise)]() mutable {
        try
        {
            action();
            promise.set_value();
        }
        catch (...)
        {
            promise.set_exception(std::current_exception());
        }
    });
    future.get();
}
