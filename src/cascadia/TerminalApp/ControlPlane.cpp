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
#include <thread>

// Use projected types only - do NOT include TermControl impl header
// (it causes WinRT vtable errors when included from TerminalApp project)

using namespace winrt::TerminalApp::implementation;
using namespace winrt::Microsoft::Terminal::Control;
using namespace winrt::Microsoft::Terminal::Settings::Model;
using namespace winrt::Windows::UI::Core;
using namespace winrt::Windows::Foundation;

namespace
{
    constexpr std::string_view kControlPlaneEnabledEnv{ "WINDOWS_TERMINAL_CONTROL_PLANE" };
    constexpr std::string_view kWin32ControlPlaneEnabledEnv{ "WINDOWS_TERMINAL_WIN32_CONTROL_PLANE" };
    constexpr std::string_view kSessionNameEnv{ "WINDOWS_TERMINAL_SESSION_NAME" };
    constexpr std::string_view kPipePrefix{ "windows-terminal-winui3-" };

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

    // ──── DLL function pointer types ────
    using CpServerCreateFn  = void* (*)(const char* session_name, const char* pipe_prefix, const void* vtable);
    using CpServerStartFn   = int   (*)(void* server);
    using CpServerStopFn    = void  (*)(void* server);
    using CpServerDestroyFn = void  (*)(void* server);
}

// ──── VTable callback trampolines (extern "C" calling convention) ────
// These are free functions with C linkage that forward to the ControlPlane
// instance passed via the ctx pointer.

extern "C" {

static size_t cpVt_readBuffer(void* ctx, char* buf, size_t buf_len)
{
    auto* cp = static_cast<ControlPlane*>(ctx);
    auto content = cp->captureTailContent(0);
    const auto copyLen = std::min(content.size(), buf_len);
    std::memcpy(buf, content.data(), copyLen);
    return copyLen;
}

static void cpVt_sendInput(void* ctx, const uint8_t* text, size_t len, bool raw)
{
    auto* cp = static_cast<ControlPlane*>(ctx);
    std::vector<uint8_t> payload(text, text + len);
    cp->enqueueInput("dll", std::move(payload), raw);
    cp->drainPendingInputs();
}

static size_t cpVt_tabCount(void* ctx)
{
    auto* cp = static_cast<ControlPlane*>(ctx);
    return cp->captureTabCount();
}

static size_t cpVt_activeTab(void* ctx)
{
    auto* cp = static_cast<ControlPlane*>(ctx);
    return cp->captureActiveTab();
}

static void cpVt_switchTab(void* ctx, size_t index)
{
    auto* cp = static_cast<ControlPlane*>(ctx);
    cp->doSwitchTab(index);
}

static void cpVt_newTab(void* ctx)
{
    auto* cp = static_cast<ControlPlane*>(ctx);
    cp->doNewTab();
}

static void cpVt_closeTab(void* ctx, size_t index)
{
    auto* cp = static_cast<ControlPlane*>(ctx);
    cp->doCloseTab(index);
}

static void cpVt_focus(void* ctx)
{
    auto* cp = static_cast<ControlPlane*>(ctx);
    cp->doFocus();
}

static size_t cpVt_hwnd(void* ctx)
{
    auto* cp = static_cast<ControlPlane*>(ctx);
    return reinterpret_cast<size_t>(cp->getHwnd());
}

static size_t cpVt_tabTitle(void* ctx, size_t index, char* buf, size_t buf_len)
{
    auto* cp = static_cast<ControlPlane*>(ctx);
    auto title = cp->captureTabTitle(index);
    const auto copyLen = std::min(title.size(), buf_len);
    std::memcpy(buf, title.data(), copyLen);
    return copyLen;
}

static size_t cpVt_tabWorkingDir(void* ctx, size_t index, char* buf, size_t buf_len)
{
    auto* cp = static_cast<ControlPlane*>(ctx);
    auto dir = cp->captureTabWorkingDir(index);
    const auto copyLen = std::min(dir.size(), buf_len);
    std::memcpy(buf, dir.data(), copyLen);
    return copyLen;
}

static bool cpVt_tabHasSelection(void* ctx, size_t index)
{
    auto* cp = static_cast<ControlPlane*>(ctx);
    return cp->captureTabHasSelection(index);
}

static size_t cpVt_readBufferForTab(void* ctx, size_t tab_index, char* buf, size_t buf_len)
{
    auto* cp = static_cast<ControlPlane*>(ctx);
    auto content = cp->captureTailContentForTab(tab_index);
    if (content.empty())
    {
        return 0;
    }
    const auto copyLen = std::min(content.size(), buf_len);
    std::memcpy(buf, content.data(), copyLen);
    return copyLen;
}

static void cpVt_sendInputToTab(void* ctx, const uint8_t* text, size_t len, bool raw, size_t tab_index)
{
    auto* cp = static_cast<ControlPlane*>(ctx);
    std::vector<uint8_t> payload(text, text + len);
    cp->sendInputToTab(std::move(payload), raw, tab_index);
}

} // extern "C"

bool ControlPlane::IsEnabled()
{
    const auto flag1 = getEnvVar(kControlPlaneEnabledEnv.data());
    const auto flag2 = getEnvVar(kWin32ControlPlaneEnabledEnv.data());

    // Debug: write diagnostic
    {
        const auto localApp = getEnvVar("LOCALAPPDATA");
        if (localApp)
        {
            std::filesystem::path diagPath(*localApp);
            diagPath /= L"WindowsTerminal";
            std::filesystem::create_directories(diagPath);
            diagPath /= L"control-plane-diag.log";
            std::ofstream diag(diagPath, std::ios::app);
            diag << "IsEnabled: flag1=" << flag1.value_or("(not set)")
                 << " flag2=" << flag2.value_or("(not set)") << "\n";
            diag.flush();
        }
    }

    if (flag1 && isTruthy(*flag1)) return true;
    if (flag2 && isTruthy(*flag2)) return true;
#ifdef WT_BRANDING_DEV
    // Dev builds: enable control plane by default for testing
    return true;
#else
    return false;
#endif
}

ControlPlane::ControlPlane(TerminalPage& page) :
    _page(page),
    _dispatcher(_page.Dispatcher()),
    _pid(static_cast<size_t>(GetCurrentProcessId()))
{
    // Debug: write diagnostic to a known location regardless of dispatcher state
    {
        const auto localApp = getEnvVar("LOCALAPPDATA");
        if (localApp)
        {
            std::filesystem::path diagPath(*localApp);
            diagPath /= L"WindowsTerminal";
            std::filesystem::create_directories(diagPath);
            diagPath /= L"control-plane-diag.log";
            std::ofstream diag(diagPath, std::ios::app);
            diag << "ControlPlane ctor: pid=" << _pid
                 << " dispatcher=" << (_dispatcher ? "OK" : "NULL")
                 << " env=" << getEnvVar(kControlPlaneEnabledEnv.data()).value_or("(not set)")
                 << "\n";
            diag.flush();
        }
    }
    if (!_dispatcher)
    {
        return;
    }

    _sessionName = getEnvVar(kSessionNameEnv.data()).value_or("");
    if (_sessionName.empty())
    {
        _sessionName = "winui3-" + std::to_string(_pid);
    }
    _safeSessionName = sanitizeSessionName(_sessionName);
    _pipeName = std::string{ kPipePrefix } + _safeSessionName + "-" + std::to_string(_pid);
    _pipePath = "\\\\.\\pipe\\" + _pipeName;
    _hwnd = _page.HostingWindow().value_or(nullptr);
    _page._SetControlPlaneTabTitleSuffix(winrt::to_hstring(fmt::format("[cp:{}-{}]", _safeSessionName, _pid)));

    try
    {
        TraceLoggingWrite(g_hTerminalAppProvider, "ControlPlaneInitStep", TraceLoggingString("dll-load:start", "Step"));
        initDll();
        TraceLoggingWrite(g_hTerminalAppProvider, "ControlPlaneInitStep", TraceLoggingString("dll-load:done", "Step"));
    }
    catch (const std::exception& e)
    {
        TraceLoggingWrite(g_hTerminalAppProvider,
                          "ControlPlaneInitFailed",
                          TraceLoggingString(e.what(), "Reason"));
        appendDiagLog(std::string("ControlPlane init failed: ") + e.what());
    }
}

ControlPlane::~ControlPlane()
{
    _stop.store(true);
    _cancelled->store(true);

    if (_dllServer)
    {
        if (_fnStop) _fnStop(_dllServer);
        if (_fnDestroy) _fnDestroy(_dllServer);
        _dllServer = nullptr;
    }
    if (_dllHandle)
    {
        FreeLibrary(_dllHandle);
        _dllHandle = nullptr;
    }
}

void ControlPlane::initDll()
{
    // Try to load control_plane_server.dll from same directory as exe
    _dllHandle = LoadLibraryW(L"control_plane_server.dll");
    if (!_dllHandle)
    {
        const auto err = GetLastError();
        appendDiagLog("LoadLibrary(control_plane_server.dll) failed, error=" + std::to_string(err) + " — control plane disabled");
        TraceLoggingWrite(g_hTerminalAppProvider, "ControlPlaneDllNotFound",
                          TraceLoggingUInt32(static_cast<DWORD>(err), "Error"));
        return; // Graceful fallback: control plane simply disabled
    }

    auto fnCreate  = reinterpret_cast<CpServerCreateFn>(GetProcAddress(_dllHandle, "cp_server_create_with_prefix"));
    auto fnStart   = reinterpret_cast<CpServerStartFn>(GetProcAddress(_dllHandle, "cp_server_start"));
    _fnStop    = reinterpret_cast<CpServerStopFn>(GetProcAddress(_dllHandle, "cp_server_stop"));
    _fnDestroy = reinterpret_cast<CpServerDestroyFn>(GetProcAddress(_dllHandle, "cp_server_destroy"));

    if (!fnCreate || !fnStart || !_fnStop || !_fnDestroy)
    {
        appendDiagLog("GetProcAddress failed for one or more DLL exports — control plane disabled");
        FreeLibrary(_dllHandle);
        _dllHandle = nullptr;
        return;
    }

    // Build the VTable — C ABI struct matching TerminalProviderVTable in ffi.rs
    // Layout must match the Rust repr(C) struct exactly:
    //   read_buffer, send_input, tab_count, active_tab, switch_tab,
    //   new_tab, close_tab, focus, hwnd, tab_title, tab_working_dir,
    //   tab_has_selection, read_buffer_for_tab, ctx
    struct TerminalProviderVTable
    {
        decltype(&cpVt_readBuffer)      read_buffer;
        decltype(&cpVt_sendInput)       send_input;
        decltype(&cpVt_tabCount)        tab_count;
        decltype(&cpVt_activeTab)       active_tab;
        decltype(&cpVt_switchTab)       switch_tab;
        decltype(&cpVt_newTab)          new_tab;
        decltype(&cpVt_closeTab)        close_tab;
        decltype(&cpVt_focus)           focus;
        decltype(&cpVt_hwnd)            hwnd;
        decltype(&cpVt_tabTitle)        tab_title;
        decltype(&cpVt_tabWorkingDir)   tab_working_dir;
        decltype(&cpVt_tabHasSelection) tab_has_selection;
        decltype(&cpVt_readBufferForTab) read_buffer_for_tab;
        decltype(&cpVt_sendInputToTab)  send_input_to_tab;
        void*                           ctx;
    };

    TerminalProviderVTable vtable{};
    vtable.read_buffer      = cpVt_readBuffer;
    vtable.send_input       = cpVt_sendInput;
    vtable.tab_count        = cpVt_tabCount;
    vtable.active_tab       = cpVt_activeTab;
    vtable.switch_tab       = cpVt_switchTab;
    vtable.new_tab          = cpVt_newTab;
    vtable.close_tab        = cpVt_closeTab;
    vtable.focus            = cpVt_focus;
    vtable.hwnd             = cpVt_hwnd;
    vtable.tab_title        = cpVt_tabTitle;
    vtable.tab_working_dir  = cpVt_tabWorkingDir;
    vtable.tab_has_selection = cpVt_tabHasSelection;
    vtable.read_buffer_for_tab = cpVt_readBufferForTab;
    vtable.send_input_to_tab   = cpVt_sendInputToTab;
    vtable.ctx              = static_cast<void*>(this);

    _dllServer = fnCreate(_sessionName.c_str(), "windows-terminal-winui3", &vtable);
    if (!_dllServer)
    {
        appendDiagLog("cp_server_create returned null — control plane disabled");
        FreeLibrary(_dllHandle);
        _dllHandle = nullptr;
        return;
    }

    const auto startResult = fnStart(_dllServer);
    if (startResult != 0)
    {
        appendDiagLog("cp_server_start failed with code=" + std::to_string(startResult));
        _fnDestroy(_dllServer);
        _dllServer = nullptr;
        FreeLibrary(_dllHandle);
        _dllHandle = nullptr;
        return;
    }

    appendDiagLog("control-plane-dll: started successfully, pipe=" + _pipeName);
}

void ControlPlane::appendDiagLog(const std::string& message)
{
    const auto localApp = getEnvVar("LOCALAPPDATA");
    if (localApp)
    {
        std::filesystem::path diagPath(*localApp);
        diagPath /= L"WindowsTerminal";
        std::filesystem::create_directories(diagPath);
        diagPath /= L"control-plane-diag.log";
        std::ofstream diag(diagPath, std::ios::app);
        diag << message << "\n";
        diag.flush();
    }
}

// ──── Public methods called from VTable callbacks ────

std::string ControlPlane::captureTailContent(size_t /*lines*/) const
{
    return runOnUiThread<std::string>([this]() -> std::string {
        const auto control = getActiveControl(std::nullopt);
        if (!control)
        {
            return {};
        }
        return toUtf8(control.ReadEntireBuffer());
    });
}

std::string ControlPlane::captureTailContentForTab(size_t tabIndex) const
{
    return runOnUiThread<std::string>([this, tabIndex]() -> std::string {
        const auto control = getActiveControl(tabIndex);
        if (!control)
        {
            return {};
        }
        return toUtf8(control.ReadEntireBuffer());
    });
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
                control.SendInput(winrt::hstring(text));
            }
            else
            {
                if (control.BracketedPasteEnabled())
                {
                    control.SendInput(winrt::hstring(L"\x1b[200~"));
                    control.SendInput(winrt::hstring(text));
                    control.SendInput(winrt::hstring(L"\x1b[201~"));
                }
                else
                {
                    control.SendInput(winrt::hstring(text));
                }
            }
        }
    });
}

void ControlPlane::sendInputToTab(std::vector<uint8_t> payload, bool raw, size_t tabIndex)
{
    runVoidOnUiThread([this, payload = std::move(payload), raw, tabIndex]() {
        const auto control = getActiveControl(tabIndex);
        if (!control)
        {
            return;
        }
        const auto text = fromUtf8(std::string_view(
            reinterpret_cast<const char*>(payload.data()),
            payload.size()));
        if (raw)
        {
            control.SendInput(winrt::hstring(text));
        }
        else
        {
            if (control.BracketedPasteEnabled())
            {
                control.SendInput(winrt::hstring(L"\x1b[200~"));
                control.SendInput(winrt::hstring(text));
                control.SendInput(winrt::hstring(L"\x1b[201~"));
            }
            else
            {
                control.SendInput(winrt::hstring(text));
            }
        }
    });
}

size_t ControlPlane::captureTabCount()
{
    return runOnUiThread<size_t>([this]() -> size_t {
        return _page.NumberOfTabs();
    });
}

size_t ControlPlane::captureActiveTab()
{
    return runOnUiThread<size_t>([this]() -> size_t {
        return _page._GetFocusedTabIndex().value_or(0);
    });
}

void ControlPlane::doSwitchTab(size_t index)
{
    runVoidOnUiThread([this, index]() {
        _page._SelectTab(static_cast<uint32_t>(index));
    });
}

void ControlPlane::doNewTab()
{
    runVoidOnUiThread([this]() {
        _page._OpenNewTab(nullptr);
    });
}

void ControlPlane::doCloseTab(size_t index)
{
    runVoidOnUiThread([this, index]() {
        const auto tabCount = _page._tabs.Size();
        if (tabCount > 0 && index < tabCount)
        {
            _page._CloseTabAtIndex(static_cast<uint32_t>(index));
        }
    });
}

void ControlPlane::doFocus()
{
    if (_hwnd)
    {
        SetForegroundWindow(_hwnd);
    }
}

HWND ControlPlane::getHwnd() const
{
    return _hwnd;
}

std::string ControlPlane::captureTabTitle(size_t index)
{
    return runOnUiThread<std::string>([this, index]() -> std::string {
        if (const auto tab = getTabImpl(static_cast<size_t>(index)))
        {
            return toUtf8(tab.value()->Title());
        }
        return {};
    });
}

std::string ControlPlane::captureTabWorkingDir(size_t index)
{
    return runOnUiThread<std::string>([this, index]() -> std::string {
        if (const auto tab = getTabImpl(static_cast<size_t>(index)))
        {
            const auto control = tab.value()->GetActiveTerminalControl();
            return toUtf8(control.WorkingDirectory());
        }
        return {};
    });
}

bool ControlPlane::captureTabHasSelection(size_t index)
{
    return runOnUiThread<bool>([this, index]() -> bool {
        if (const auto tab = getTabImpl(static_cast<size_t>(index)))
        {
            const auto control = tab.value()->GetActiveTerminalControl();
            return control.HasSelection();
        }
        return false;
    });
}

// ──── Private helpers (retained from original) ────

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

template<typename TResult>
TResult ControlPlane::runOnUiThread(std::function<TResult()> action) const
{
    if (_stop.load())
    {
        return TResult{};
    }
    std::promise<TResult> promise;
    auto future = promise.get_future();
    auto cancelled = _cancelled;
    try
    {
        _dispatcher.RunAsync(CoreDispatcherPriority::Normal, [cancelled, action = std::move(action), promise = std::move(promise)]() mutable {
            try
            {
                if (cancelled->load())
                {
                    promise.set_value(TResult{});
                    return;
                }
                promise.set_value(action());
            }
            catch (...)
            {
                promise.set_exception(std::current_exception());
            }
        });
    }
    catch (...)
    {
        return TResult{};
    }
    if (future.wait_for(std::chrono::seconds(5)) == std::future_status::ready)
    {
        return future.get();
    }
    return TResult{};
}

void ControlPlane::runVoidOnUiThread(std::function<void()> action) const
{
    if (_stop.load())
    {
        return;
    }
    std::promise<void> promise;
    auto future = promise.get_future();
    auto cancelled = _cancelled;
    try
    {
        _dispatcher.RunAsync(CoreDispatcherPriority::Normal, [cancelled, action = std::move(action), promise = std::move(promise)]() mutable {
            try
            {
                if (cancelled->load())
                {
                    promise.set_value();
                    return;
                }
                action();
                promise.set_value();
            }
            catch (...)
            {
                promise.set_exception(std::current_exception());
            }
        });
    }
    catch (...)
    {
        return;
    }
    future.wait_for(std::chrono::seconds(5));
}
