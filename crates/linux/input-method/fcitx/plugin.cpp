// Fcitx5 系统胶水：只转发有界请求和绘制框架面板，词表、键盘规则和业务都在 Rust 后台。
#include <gio/gio.h>
#include <fcitx/addonfactory.h>
#include <fcitx/addonmanager.h>
#include <fcitx/action.h>
#include <fcitx/candidatelist.h>
#include <fcitx/inputcontext.h>
#include <fcitx/inputcontextmanager.h>
#include <fcitx/inputcontextproperty.h>
#include <fcitx/inputmethodengine.h>
#include <fcitx/inputpanel.h>
#include <fcitx/instance.h>
#include <fcitx/statusarea.h>
#include <fcitx/userinterfacemanager.h>
#include <array>
#include <memory>
#include <limits>
#include <stdexcept>
#include <string>
#include <string_view>
#include <vector>

namespace {
using Variant = std::unique_ptr<GVariant, decltype(&g_variant_unref)>;
constexpr const char *service = "local.tigertype.Fcitx";
constexpr const char *path = "/local/tigertype/Fcitx";
constexpr const char *interface = "local.tigertype.Fcitx1";
// 基础菜单的固定名字与后台 menu::BASE 一致；前三项是模式切换，后两项打开窗口。
constexpr std::array<const char *, 5> baseNames = {
    "tigertype-ascii", "tigertype-shape", "tigertype-punctuation", "tigertype-lookup", "tigertype-settings"};
constexpr std::array<const char *, 5> baseLabels = {"中文", "半角", "中文标点", "查字", "设置"};
class Engine;
struct State : fcitx::InputContextProperty {
    Engine *engine;
    uint64_t id, sequence = 0;
    bool active = false, sensitive = false;
    std::array<std::string, 5> labels{baseLabels[0], baseLabels[1], baseLabels[2], baseLabels[3], baseLabels[4]};
    std::vector<std::unique_ptr<fcitx::Action>> actions;
    State(Engine *owner, uint64_t number) : engine(owner), id(number) {}
    ~State() override;
};
// 常驻基础菜单：引擎持有、名字固定，焦点切换和重绘都不重建，托盘菜单延后激活时编号仍然有效。
class BaseAction final : public fcitx::Action {
    Engine *engine_; size_t index_;
public:
    BaseAction(Engine *engine, size_t index) : engine_(engine), index_(index) {}
    std::string shortText(fcitx::InputContext *ic) const override;
    std::string icon(fcitx::InputContext *) const override { return {}; }
    void activate(fcitx::InputContext *ic) override;
};
class Engine final : public fcitx::InputMethodEngineV2 {
    fcitx::Instance *instance_;
    GDBusConnection *bus_ = nullptr;
    std::string owner_;
    uint64_t next_ = 0;
    std::vector<std::unique_ptr<BaseAction>> base_;
    // 工厂最后销毁之前主动注销，确保上下文释放时后台连接仍有效。
    fcitx::FactoryFor<State> factory_;
    Variant call(const char *destination, const char *object, const char *iface,
                 const char *method, GVariant *args, const GVariantType *result, gint64 deadline) {
        const auto left = deadline - g_get_monotonic_time();
        if (!bus_ || left <= 0) throw std::runtime_error("deadline");
        GError *error = nullptr;
        Variant value(g_dbus_connection_call_sync(bus_, destination, object, iface, method, args, result,
            G_DBUS_CALL_FLAGS_NO_AUTO_START, static_cast<int>((left + 999) / 1000), nullptr, &error), g_variant_unref);
        if (error) { g_error_free(error); throw std::runtime_error("dbus"); }
        if (!value || g_variant_get_size(value.get()) > 65536 || g_get_monotonic_time() >= deadline)
            throw std::runtime_error("late or oversized reply");
        return value;
    }
    void resolve(gint64 deadline) {
        auto reply = call("org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus", "GetNameOwner",
            g_variant_new("(s)", service), G_VARIANT_TYPE("(s)"), deadline);
        const gchar *name = nullptr;
        g_variant_get(reply.get(), "(&s)", &name);
        if (!name || name[0] != ':') throw std::runtime_error("owner");
        owner_ = name;
    }
    // 撤下面板与快照动作，基础菜单留在状态区；框架在激活前清空本组时由下一次重绘补回。
    void clear(fcitx::InputContext *ic) {
        ic->inputPanel().reset();
        ic->statusArea().clearGroup(fcitx::StatusGroup::InputMethod);
        ic->propertyFor(&factory_)->actions.clear();
        for (auto &action : base_) ic->statusArea().addAction(fcitx::StatusGroup::InputMethod, action.get());
        ic->updatePreedit();
        ic->updateUserInterface(fcitx::UserInterfaceComponent::InputPanel);
        ic->updateUserInterface(fcitx::UserInterfaceComponent::StatusArea);
    }
    // 复位只撤销组合、保留后台会话与中英等模式；失败时标为失效，下次按键重建上下文。
    void cancel(State &state) noexcept {
        if (!state.active) return;
        try { request(state, "reset", 0, 0, "", g_get_monotonic_time() + 50000); } catch (...) { state.active = false; }
    }
public:
    explicit Engine(fcitx::Instance *instance) : instance_(instance), factory_([this](fcitx::InputContext &) {
        if (next_ == std::numeric_limits<uint64_t>::max()) throw std::runtime_error("context exhausted");
        return new State(this, ++next_);
    }) {
        for (size_t i = 0; i < baseNames.size(); ++i) {
            auto action = std::make_unique<BaseAction>(this, i);
            if (!action->registerAction(baseNames[i], &instance_->userInterfaceManager()))
                throw std::runtime_error("base action");
            base_.push_back(std::move(action));
        }
        if (!instance_->inputContextManager().registerProperty("tigertypeState", &factory_))
            throw std::runtime_error("property");
        // 唯一启动期连接；输入路径不启动程序、不读配置、不按键创建线程。
        GError *error = nullptr;
        bus_ = g_bus_get_sync(G_BUS_TYPE_SESSION, nullptr, &error);
        if (error) g_error_free(error);
    }
    ~Engine() override {
        factory_.unregister();
        if (bus_) g_object_unref(bus_);
    }
    Variant request(State &state, const char *command, uint32_t symbol, uint32_t modifiers, const std::string &token, gint64 deadline) {
        if (state.sequence == std::numeric_limits<uint64_t>::max()) throw std::runtime_error("sequence exhausted");
        const auto sequence = ++state.sequence;
        auto reply = call(owner_.c_str(), path, interface, "Event", g_variant_new("(uttxsuus)", 1u, state.id, sequence,
            deadline, command, symbol, modifiers, token.c_str()), G_VARIANT_TYPE("(utbssa(ss)isa(ss))"), deadline);
        guint32 version = 0; guint64 answer = 0;
        g_variant_get_child(reply.get(), 0, "u", &version);
        g_variant_get_child(reply.get(), 1, "t", &answer);
        if (version != 1 || answer != sequence) throw std::runtime_error("protocol mismatch");
        return reply;
    }
    void close(State &state) noexcept {
        // 上下文销毁时释放后台会话；即使 active 已因失败清除也要发送，避免后台上下文堆积到上限。
        try { if (state.sequence && !owner_.empty()) request(state, "close", 0, 0, "", g_get_monotonic_time() + 50000); } catch (...) {}
        state.active = false;
    }
    std::string label(fcitx::InputContext *ic, size_t index) noexcept {
        // 框架绘制菜单时调入，异常不能越过插件边界；取不到上下文状态就显示默认文字。
        try { if (ic) return ic->propertyFor(&factory_)->labels[index]; } catch (...) {}
        return baseLabels[index];
    }
    // 托盘菜单常在焦点回到应用前激活：窗口命令不要求焦点；模式命令由后台要求空组合与已有会话。
    void base(fcitx::InputContext *ic, size_t index) noexcept {
        try {
            if (!ic || index >= baseNames.size()) return;
            auto &state = *ic->propertyFor(&factory_);
            const auto deadline = g_get_monotonic_time() + 50000;
            if (owner_.empty()) resolve(deadline);
            auto reply = request(state, "base", 0, 0, baseNames[index], deadline);
            if (index < 3) render(ic, state, reply.get(), deadline);
        } catch (...) {
            // 后台可能已重启换名；下次重新解析，不重放本次动作。
            owner_.clear();
        }
    }
    bool render(fcitx::InputContext *ic, State &state, GVariant *reply, gint64 deadline);
    bool event(fcitx::InputContext *ic, const char *command, uint32_t symbol, uint32_t modifiers, std::string token = "") noexcept {
        bool committed = false;
        try {
            if (!ic || !ic->hasFocus()) return false;
            auto &state = *ic->propertyFor(&factory_);
            const auto deadline = g_get_monotonic_time() + 50000;
            if (!state.active) {
                resolve(deadline);
                // 新建或失效后重建后台上下文；本次就是焦点请求时不重复发送。
                if (std::string_view(command) != "focus") request(state, "focus", 0, 0, "", deadline);
            }
            auto reply = request(state, command, symbol, modifiers, token, deadline);
            state.active = true;
            // 先记住是否含提交，避免已提交后绘制异常又把原键放行。
            const gchar *commit = nullptr;
            g_variant_get_child(reply.get(), 3, "&s", &commit);
            if (g_get_monotonic_time() >= deadline) throw std::runtime_error("late commit");
            if (commit && *commit) { ic->commitString(commit); committed = true; }
            return render(ic, state, reply.get(), deadline) || committed;
        } catch (...) {
            if (ic) {
                try { auto &state = *ic->propertyFor(&factory_); state.active = false; clear(ic); } catch (...) {}
            }
            return committed;
        }
    }
    void keyEvent(const fcitx::InputMethodEntry &, fcitx::KeyEvent &key) override {
        // 密码和敏感输入交还宿主，只在进入时撤销一次组合，不把这些按键送往后台。
        try {
            auto *ic = key.inputContext();
            auto &state = *ic->propertyFor(&factory_);
            const bool sensitive = ic->capabilityFlags().testAny(fcitx::CapabilityFlag::PasswordOrSensitive);
            if (sensitive && !state.sensitive) { clear(ic); cancel(state); }
            state.sensitive = sensitive;
            if (sensitive) return;
        } catch (...) { return; }
        if (event(key.inputContext(), "key", key.rawKey().sym(), static_cast<uint32_t>(key.rawKey().states().toInteger()) | (key.isRelease() ? 1u << 30 : 0))) key.filterAndAccept();
    }
    void activate(const fcitx::InputMethodEntry &, fcitx::InputContextEvent &event) override {
        this->event(event.inputContext(), "focus", 0, 0);
    }
    // 框架复位和失焦（默认 deactivate 即复位）都走这里：撤销组合但不关闭会话，基础菜单保留。
    void reset(const fcitx::InputMethodEntry &, fcitx::InputContextEvent &event) override {
        try { auto *ic = event.inputContext(); clear(ic); cancel(*ic->propertyFor(&factory_)); } catch (...) {}
    }
    fcitx::UserInterfaceManager *ui() { return &instance_->userInterfaceManager(); }
};
State::~State() { engine->close(*this); }
std::string BaseAction::shortText(fcitx::InputContext *ic) const { return engine_->label(ic, index_); }
void BaseAction::activate(fcitx::InputContext *ic) { engine_->base(ic, index_); }
class MenuAction final : public fcitx::Action {
    Engine *engine_; std::string token_, text_;
public:
    MenuAction(Engine *engine, std::string token, std::string text) : engine_(engine), token_(std::move(token)), text_(std::move(text)) {}
    std::string shortText(fcitx::InputContext *) const override { return text_; }
    std::string icon(fcitx::InputContext *) const override { return {}; }
    void activate(fcitx::InputContext *ic) override {
        // 菜单重绘可能释放本对象，先复制令牌，之后不再读取 this。
        auto *engine = engine_; auto token = token_; engine->event(ic, "menu", 0, 0, std::move(token));
    }
};
class Candidate final : public fcitx::CandidateWord {
    Engine *engine_; uint32_t index_;
public:
    // 合并注释以兼容首批目标中较早的 Fcitx5，不依赖 5.1.9 才增加的 setComment。
    Candidate(Engine *engine, uint32_t index, const char *text, const char *comment) : fcitx::CandidateWord(fcitx::Text(std::string(text) + (*comment ? "  " : "") + comment)), engine_(engine), index_(index) {}
    void select(fcitx::InputContext *ic) const override {
        // 原生序号没有展示编号，只请求带文字的确认菜单，绝不在此直接提交。
        auto *engine = engine_; auto index = index_; engine->event(ic, "confirm", index, 0);
    }
};
bool Engine::render(fcitx::InputContext *ic, State &state, GVariant *reply, gint64) {
    gboolean handled = false; const gchar *commit = nullptr, *preedit = nullptr, *caption = nullptr;
    GVariant *candidates = nullptr, *menus = nullptr; gint32 cursor = -1; guint32 version; guint64 sequence;
    g_variant_get(reply, "(utb&s&s@a(ss)i&s@a(ss))", &version, &sequence, &handled, &commit, &preedit, &candidates, &cursor, &caption, &menus);
    Variant list(candidates, g_variant_unref), menu(menus, g_variant_unref);
    if (g_variant_n_children(candidates) > 9 || g_variant_n_children(menus) > 20) throw std::runtime_error("bounds");
    // 预编辑放在框架面板，不让 toolkit 在失焦时隐式提交尚未选定的编码。
    clear(ic);
    ic->inputPanel().setPreedit(fcitx::Text(preedit));
    ic->inputPanel().setAuxDown(fcitx::Text(caption));
    auto words = std::make_unique<fcitx::CommonCandidateList>();
    words->setPageSize(9); words->setLayoutHint(fcitx::CandidateLayoutHint::Vertical);
    words->setLabels({"1", "2", "3", "4", "5", "6", "7", "8", "9"});
    for (gsize i = 0; i < g_variant_n_children(candidates); ++i) {
        const gchar *text = nullptr, *comment = nullptr;
        g_variant_get_child(candidates, i, "(&s&s)", &text, &comment);
        words->append(std::make_unique<Candidate>(this, i, text, comment));
    }
    if (cursor >= 0 && cursor < words->size()) words->setGlobalCursorIndex(cursor);
    ic->inputPanel().setCandidateList(std::move(words));
    for (gsize i = 0; i < g_variant_n_children(menus); ++i) {
        const gchar *key = nullptr, *text = nullptr;
        g_variant_get_child(menus, i, "(&s&s)", &key, &text);
        // 前五项是基础菜单的当前文字：只更新常驻动作的标签，不按每次展示的名字重建。
        if (i < base_.size()) {
            if (state.labels[i] != text) { state.labels[i] = text; base_[i]->update(ic); }
            continue;
        }
        auto action = std::make_unique<MenuAction>(this, key, text);
        if (!action->registerAction(key, ui())) throw std::runtime_error("action registration");
        ic->statusArea().addAction(fcitx::StatusGroup::InputMethod, action.get());
        state.actions.push_back(std::move(action));
    }
    ic->updateUserInterface(fcitx::UserInterfaceComponent::InputPanel);
    ic->updateUserInterface(fcitx::UserInterfaceComponent::StatusArea);
    return handled;
}
class Factory final : public fcitx::AddonFactory {
    fcitx::AddonInstance *create(fcitx::AddonManager *manager) override {
        // 构造异常止于平台工厂；不跨越插件调用边界。
        try { return new Engine(manager->instance()); } catch (...) { return nullptr; }
    }
};
}
FCITX_ADDON_FACTORY(Factory);
