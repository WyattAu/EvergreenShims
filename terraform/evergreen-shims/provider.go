package main

import (
	"context"

	"github.com/hashicorp/terraform-plugin-sdk/v2/diag"
	"github.com/hashicorp/terraform-plugin-sdk/v2/helper/schema"
	"k8s.io/client-go/kubernetes"
	"k8s.io/client-go/rest"
	"k8s.io/client-go/tools/clientcmd"
)

func Provider() *schema.Provider {
	return &schema.Provider{
		Schema: map[string]*schema.Schema{
			"kubeconfig_path": {
				Type:        schema.TypeString,
				Optional:    true,
				DefaultFunc: schema.EnvDefaultFunc("KUBECONFIG", ""),
				Description: "Path to kubeconfig file. If not set, uses in-cluster config.",
			},
			"kubeconfig_context": {
				Type:        schema.TypeString,
				Optional:    true,
				Description: "Context to use from kubeconfig.",
			},
			"namespace": {
				Type:        schema.TypeString,
				Optional:    true,
				Default:     "default",
				Description: "Default namespace for resources.",
			},
			"management_api_endpoint": {
				Type:        schema.TypeString,
				Required:    true,
				Description: "Endpoint for the EvergreenShims management API.",
			},
		},
		ResourcesMap: map[string]*schema.Resource{
			"evergreen_shims_shim_config":  resourceShimConfig(),
			"evergreen_shims_deployment":   resourceShimDeployment(),
		},
		DataSourcesMap: map[string]*schema.Resource{
			"evergreen_shims_status": dataSourceShimStatus(),
		},
	}
}

func providerConfigure(ctx context.Context, d *schema.ResourceData) (interface{}, diag.Diagnostics) {
	var config *rest.Config
	var err error

	kubeconfigPath := d.Get("kubeconfig_path").(string)
	contextName := d.Get("kubeconfig_context").(string)

	if kubeconfigPath != "" {
		configLoadingRules := clientcmd.NewDefaultClientConfigLoadingRules()
		configLoadingRules.ExplicitPath = kubeconfigPath

		configOverrides := &clientcmd.ConfigOverrides{}
		if contextName != "" {
			configOverrides.CurrentContext = contextName
		}

		config, err = clientcmd.BuildConfigFromRules("", configLoadingRules.ConfigFile)
		if err != nil {
			return nil, diag.Errorf("failed to load kubeconfig: %v", err)
		}
	} else {
		config, err = rest.InClusterConfig()
		if err != nil {
			return nil, diag.Errorf("failed to create in-cluster config: %v", err)
		}
	}

	clientset, err := kubernetes.NewForConfig(config)
	if err != nil {
		return nil, diag.Errorf("failed to create Kubernetes client: %v", err)
	}

	return &ProviderConfig{
		Clientset:             clientset,
		Namespace:             d.Get("namespace").(string),
		ManagementAPIEndpoint: d.Get("management_api_endpoint").(string),
	}, nil
}

type ProviderConfig struct {
	Clientset             *kubernetes.Clientset
	Namespace             string
	ManagementAPIEndpoint string
}
