module.exports = {
  apps: [
    {
      name: "synalux-local-relay",
      script: "./server.mjs",
      watch: false,
      env: {
        NODE_ENV: "production",
      },
      // Automatically restart if it crashes
      autorestart: true,
      // Restart up to 10 times if crashing rapidly
      max_restarts: 10,
    }
  ]
};
