export const routes = [
  {
    path: "/",
    redirect: "/home",
  },

  {
    path: "/home",
    name: "home",
    component: () => import("../views/HomeView.vue"),
  },

  {
    path: "/search",
    name: "search",
    component: () => import("../views/SearchView.vue"),
  },

  {
    path: "/playlist/:id",
    name: "playlist",
    component: () => import("../views/PlaylistView.vue"),
    props: true,
  },

  {
    path: "/album/:id",
    name: "album",
    component: () => import("../views/AlbumView.vue"),
    props: true,
  },

  {
    path: "/artist/:id",
    name: "artist",
    component: () => import("../views/ArtistView.vue"),
    props: true,
  },

  {
    path: "/library",
    name: "library",
    component: () => import("../views/library/LibraryView.vue"),
  },

  {
    path: "/library/recent",
    name: "recent",
    component: () => import("../views/library/RecentView.vue"),
  },

  {
    path: "/now-playing",
    name: "now-playing",
    component: () => import("../views/NowPlayingView.vue"),
  },

  {
    path: "/settings",
    name: "settings",
    component: () => import("../views/settings/SettingsView.vue"),
  },

  {
    path: "/settings/general",
    name: "settings-general",
    component: () => import("../views/settings/GeneralSettingsView.vue"),
  },

  {
    path: "/settings/playback",
    name: "settings-playback",
    component: () => import("../views/settings/PlaybackSettingsView.vue"),
  },

  {
    path: "/settings/account",
    name: "settings-account",
    component: () => import("../views/settings/AccountSettingsView.vue"),
  },
];
